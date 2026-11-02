//! Telemetry for services, from the `tracing` events they already emit:
//!
//! - **console**: one readable, coloured line per event on stdout:
//!   `[time][LEVEL][module:line] message  META::[ data]`, the message in the
//!   level's colour;
//! - **OpenTelemetry**: the same spans and events exported as traces and
//!   logs over OTLP, to a Collector (Grafana's stack, or any other);
//! - **propagation**: W3C trace context in and out of HTTP and gRPC
//!   headers, and through event headers.
//!
//! ```no_run
//! # async fn run() -> Result<(), z_telemetry::Error> {
//! let config = z_telemetry::TelemetryConfig::default(); // from the service's configuration
//! let telemetry = z_telemetry::init(&config, z_telemetry::Service::new("my-service", "1.2.0"))?;
//! tracing::info!(port = 8080, "serving");
//! // … run …
//! telemetry.shutdown(); // flush what is still buffered
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod console;
mod otel;
pub mod propagation;
#[cfg(test)]
mod testing;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

pub use config::{
    ColorMode, ConfigError, ConsoleConfig, OtlpConfig, Protocol, TelemetryConfig, TimeZone,
};
use console::{ConsoleFields, ConsoleFormat};

/// The service whose telemetry this is: `service.name` and
/// `service.version` on everything exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// `service.name`, e.g. `users`.
    pub name: String,
    /// `service.version`, e.g. `env!("CARGO_PKG_VERSION")`.
    pub version: String,
}

impl Service {
    /// A service named `name` at `version`.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

/// Telemetry could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The configuration (or `RUST_LOG`) cannot be used.
    #[error("invalid telemetry configuration:\n{0}")]
    Config(#[from] ConfigError),
    /// An OTLP exporter could not be built.
    #[error("OTLP exporter: {0}")]
    Exporter(String),
    /// The configured OTLP protocol is not compiled in.
    #[error("OTLP over {0:?} is not compiled in (features `otlp-grpc`, `otlp-http`)")]
    Unsupported(Protocol),
    /// OTLP over gRPC was configured but `init` ran outside a tokio runtime.
    #[error("OTLP over gRPC needs a tokio runtime: call init inside one")]
    NoRuntime,
    /// Another global `tracing` subscriber is installed already.
    #[error("a global tracing subscriber is installed already")]
    AlreadyInitialized,
}

/// Running telemetry. [`shutdown`](Self::shutdown) (or dropping it) flushes
/// what is still buffered: exports, then the console.
#[must_use = "dropping it shuts telemetry down"]
pub struct Telemetry {
    tracer: Option<SdkTracerProvider>,
    logger: Option<SdkLoggerProvider>,
    console: Option<WorkerGuard>,
}

impl std::fmt::Debug for Telemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Telemetry")
            .field("exporting", &self.tracer.is_some())
            .finish_non_exhaustive()
    }
}

impl Telemetry {
    /// Flush and stop the exporters, then the console writer. Blocks until
    /// the exports finish or time out.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(logger) = self.logger.take()
            && let Err(error) = logger.shutdown()
        {
            tracing::warn!(%error, "flushing OpenTelemetry logs failed");
        }
        if let Some(tracer) = self.tracer.take()
            && let Err(error) = tracer.shutdown()
        {
            tracing::warn!(%error, "flushing OpenTelemetry traces failed");
        }
        drop(self.console.take());
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.stop();
    }
}

fn filter(source: &str, what: &str) -> Result<EnvFilter, Error> {
    EnvFilter::builder()
        .parse(source)
        .map_err(|e| Error::Config(ConfigError(vec![format!("{what}: {e}")])))
}

/// The console filter: `RUST_LOG` when set, else `console.level`.
fn console_level(config: &TelemetryConfig) -> (String, &'static str) {
    match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => (v, "RUST_LOG"),
        _ => (config.console.level.clone(), "console.level"),
    }
}

/// The subscriber: the console on `writer`, and the OpenTelemetry layers
/// over `tracer` and `logger` when given.
fn compose<W>(
    config: &TelemetryConfig,
    service: &Service,
    writer: W,
    tracer: Option<&SdkTracerProvider>,
    logger: Option<&SdkLoggerProvider>,
) -> Result<impl Subscriber + Send + Sync + 'static, Error>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let (level, source) = console_level(config);
    let mut format = ConsoleFormat::new(config.console.time);
    // Always registered (an `Option` layer would not pass the dispatch on);
    // consulted only when spans are exported.
    let trace_ids = otel::TraceIds::default();
    if tracer.is_some() {
        format = format.with_trace_ids(trace_ids.clone());
    }
    let console = tracing_subscriber::fmt::layer()
        .event_format(format)
        .fmt_fields(ConsoleFields)
        .with_ansi(console::colors(config.console.color))
        .with_writer(writer)
        .with_filter(filter(&level, source)?);
    // Spans are exported at the console's level; events at `otlp.level`.
    let spans = match tracer {
        Some(provider) => Some(
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer(service.name.clone()))
                .with_filter(filter(&format!("{level},{}", otel::QUIET), source)?),
        ),
        None => None,
    };
    let logs = match (logger, &config.otlp) {
        (Some(provider), Some(otlp)) => Some(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(provider)
                .with_filter(filter(
                    &format!("{},{}", otlp.level, otel::QUIET),
                    "otlp.level",
                )?),
        ),
        _ => None,
    };
    Ok(tracing_subscriber::registry()
        .with(console)
        .with(spans)
        .with(logs)
        .with(trace_ids))
}

/// Start telemetry for `service` as `config` says, as the process's global
/// `tracing` subscriber (`log` records are forwarded into it): the console,
/// and the OTLP export when `config.otlp` is set. Call it once, early, inside
/// the tokio runtime when exporting over gRPC.
///
/// # Errors
/// [`Error`]: an unusable configuration or `RUST_LOG`, an exporter that
/// cannot be built, or a subscriber installed already.
pub fn init(config: &TelemetryConfig, service: Service) -> Result<Telemetry, Error> {
    config.validate()?;
    let (tracer, logger) = match &config.otlp {
        #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
        Some(otlp) => {
            let resource = otel::resource(&service, &config.environment);
            (
                Some(otel::tracer_provider(
                    otel::otlp::spans(otlp)?,
                    resource.clone(),
                    otlp.sample_ratio,
                )),
                Some(otel::logger_provider(otel::otlp::logs(otlp)?, resource)),
            )
        }
        #[cfg(not(any(feature = "otlp-grpc", feature = "otlp-http")))]
        Some(otlp) => return Err(Error::Unsupported(otlp.protocol)),
        None => (None, None),
    };
    let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .thread_name("z-telemetry-console")
        .finish(std::io::stdout());
    compose(config, &service, writer, tracer.as_ref(), logger.as_ref())?
        .try_init()
        .map_err(|_| Error::AlreadyInitialized)?;
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    Ok(Telemetry {
        tracer,
        logger,
        console: Some(guard),
    })
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::TraceId;
    use opentelemetry_sdk::logs::InMemoryLogExporter;
    use opentelemetry_sdk::trace::InMemorySpanExporter;

    use super::*;
    use crate::testing::Captured;

    struct Exporting {
        spans: InMemorySpanExporter,
        logs: InMemoryLogExporter,
        tracer: SdkTracerProvider,
        logger: SdkLoggerProvider,
        console: Captured,
    }

    /// Run `f` under the composed subscriber with in-memory exporters.
    fn exporting(otlp_level: &str, ratio: f64, f: impl FnOnce()) -> Exporting {
        let mut config = TelemetryConfig::default();
        config.console.color = ColorMode::Never;
        config.console.level = "debug".into();
        let mut otlp = OtlpConfig::new("http://127.0.0.1:4317");
        otlp.level = otlp_level.into();
        config.otlp = Some(otlp);
        let service = Service::new("test-service", "1.2.3");
        let resource = otel::resource(&service, "test");
        let spans = InMemorySpanExporter::default();
        let logs = InMemoryLogExporter::default();
        let tracer = otel::tracer_provider(spans.clone(), resource.clone(), ratio);
        let logger = otel::logger_provider(logs.clone(), resource);
        let console = Captured::default();
        let subscriber = compose(
            &config,
            &service,
            console.clone(),
            Some(&tracer),
            Some(&logger),
        )
        .unwrap();
        tracing::subscriber::with_default(subscriber, f);
        tracer.force_flush().unwrap();
        logger.force_flush().unwrap();
        Exporting {
            spans,
            logs,
            tracer,
            logger,
            console,
        }
    }

    #[test]
    fn spans_and_events_are_exported_and_correlated() {
        let run = exporting("info", 1.0, || {
            let span = tracing::info_span!("http", request_id = "r1");
            let _entered = span.enter();
            tracing::info!(user = "u1", "signed in");
            tracing::debug!("only on the console");
        });
        let spans = run.spans.get_finished_spans().unwrap();
        let [span] = spans.as_slice() else {
            panic!("{spans:?}")
        };
        assert_eq!(span.name, "http");

        let logs = run.logs.get_emitted_logs().unwrap();
        assert_eq!(logs.len(), 1, "debug stays on the console");
        let service = logs[0]
            .resource
            .get(&opentelemetry::Key::new("service.name"))
            .unwrap();
        assert_eq!(service.as_str(), "test-service");
        let record = &logs[0].record;
        let trace: TraceId = record.trace_context().unwrap().trace_id;
        assert_eq!(trace, span.span_context.trace_id());

        let console = run.console.text();
        let short = &trace.to_string()[..8];
        assert!(console.contains(&format!("trace={short}")), "{console}");
        assert!(console.contains("only on the console"));
        run.tracer.shutdown().unwrap();
        run.logger.shutdown().unwrap();
    }

    #[test]
    fn the_exporters_own_crates_are_never_exported() {
        let run = exporting("trace", 1.0, || {
            tracing::info!(target: "tonic::transport", "connecting");
            tracing::info!(target: "opentelemetry_sdk", "exporting");
            tracing::info!(target: "my_service", "kept");
        });
        let logs = run.logs.get_emitted_logs().unwrap();
        assert_eq!(logs.len(), 1, "{logs:?}");
    }

    #[test]
    fn unsampled_traces_are_not_exported() {
        let run = exporting("info", 0.0, || {
            let span = tracing::info_span!("job");
            let _entered = span.enter();
            tracing::info!("working");
        });
        assert!(run.spans.get_finished_spans().unwrap().is_empty());
        assert!(run.console.text().contains("working"));
    }

    #[test]
    fn invalid_settings_fail_init() {
        let mut config = TelemetryConfig::default();
        config.console.level = "info,[".into();
        assert!(matches!(
            init(&config, Service::new("x", "0")),
            Err(Error::Config(_))
        ));
        config.console.level = "info".into();
        config.otlp = Some(OtlpConfig::new("http://127.0.0.1:4317"));
        // Outside a runtime, gRPC cannot start.
        assert!(matches!(
            init(&config, Service::new("x", "0")),
            Err(Error::NoRuntime)
        ));
    }

    #[test]
    fn traces_continue_from_headers_and_events() {
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        let caller = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let mut headers = http::HeaderMap::new();
        headers.insert("traceparent", caller.parse().unwrap());
        let mut onward = http::HeaderMap::new();
        let mut from_event = None;
        let run = exporting("info", 1.0, || {
            let span = tracing::info_span!("http");
            propagation::set_parent(&span, &headers);
            let _entered = span.enter();
            propagation::inject(&span, &mut onward);
            let job = tracing::info_span!(parent: None, "event");
            propagation::set_parent_from(&job, caller);
            from_event = propagation::traceparent(&job);
        });
        let spans = run.spans.get_finished_spans().unwrap();
        let caller_trace = "4bf92f3577b34da6a3ce929d0e0e4736";
        assert_eq!(spans.len(), 2);
        for span in &spans {
            assert_eq!(span.span_context.trace_id().to_string(), caller_trace);
            assert_eq!(span.parent_span_id.to_string(), "00f067aa0ba902b7");
            assert!(span.parent_span_is_remote);
        }
        let sent = onward["traceparent"].to_str().unwrap();
        assert!(sent.starts_with(&format!("00-{caller_trace}-")), "{sent}");
        assert!(from_event.unwrap().contains(caller_trace));
    }

    #[test]
    fn without_a_traceparent_a_span_starts_its_own_trace() {
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        let mut own = None;
        let run = exporting("info", 1.0, || {
            let span = tracing::info_span!("http");
            propagation::set_parent(&span, &http::HeaderMap::new());
            own = propagation::traceparent(&span);
        });
        let spans = run.spans.get_finished_spans().unwrap();
        let trace = spans[0].span_context.trace_id().to_string();
        assert!(own.unwrap().contains(&trace));
        assert!(!spans[0].parent_span_is_remote);
    }
}
