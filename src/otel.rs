//! OpenTelemetry: spans become traces, events become logs, both exported in
//! batches over OTLP from the SDK's own threads.

use std::sync::{Arc, OnceLock};

use opentelemetry::KeyValue;
use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{LogExporter, SdkLoggerProvider};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider, SpanExporter};
use tracing::dispatcher::WeakDispatch;
use tracing::{Dispatch, Subscriber, span};
use tracing_subscriber::Layer;

use crate::Service;

/// Crates whose own spans and events are never exported: the exporters
/// themselves, so exporting never feeds itself.
pub(crate) const QUIET: &str = "opentelemetry=off,opentelemetry_sdk=off,opentelemetry_otlp=off,\
     opentelemetry_http=off,tonic=off,h2=off,hyper=off,hyper_util=off,reqwest=off,tower=off";

/// What every exported span and log record says about where it comes from.
#[cfg_attr(
    not(any(feature = "otlp-grpc", feature = "otlp-http")),
    allow(dead_code)
)]
pub(crate) fn resource(service: &Service, environment: &str) -> Resource {
    Resource::builder()
        .with_service_name(service.name.clone())
        .with_attributes([
            KeyValue::new("service.version", service.version.clone()),
            KeyValue::new("service.instance.id", uuid::Uuid::new_v4().to_string()),
            KeyValue::new("deployment.environment.name", environment.to_owned()),
        ])
        .build()
}

/// Traces: new traces sampled at `ratio`, continued ones as their caller
/// decided.
#[cfg_attr(
    not(any(feature = "otlp-grpc", feature = "otlp-http")),
    allow(dead_code)
)]
pub(crate) fn tracer_provider<E: SpanExporter + 'static>(
    exporter: E,
    resource: Resource,
    ratio: f64,
) -> SdkTracerProvider {
    SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            ratio,
        ))))
        .with_resource(resource)
        .build()
}

/// Logs.
#[cfg_attr(
    not(any(feature = "otlp-grpc", feature = "otlp-http")),
    allow(dead_code)
)]
pub(crate) fn logger_provider<E: LogExporter + 'static>(
    exporter: E,
    resource: Resource,
) -> SdkLoggerProvider {
    SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build()
}

/// Trace ids for the console. Inside an event callback `tracing` hides the
/// current dispatcher (to prevent recursion), so the span's OpenTelemetry
/// context is looked up through the subscriber this layer was registered
/// with.
#[derive(Debug, Clone, Default)]
pub(crate) struct TraceIds {
    dispatch: Arc<OnceLock<WeakDispatch>>,
}

impl TraceIds {
    /// The trace id of span `id`, when it has a valid one.
    pub(crate) fn of(&self, id: &span::Id) -> Option<String> {
        let dispatch = self.dispatch.get()?.upgrade()?;
        let context = tracing_opentelemetry::get_otel_context(id, &dispatch)?;
        let span = context.span();
        let span_context = span.span_context();
        span_context
            .is_valid()
            .then(|| span_context.trace_id().to_string())
    }
}

impl<S: Subscriber> Layer<S> for TraceIds {
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        let _ = self.dispatch.set(subscriber.downgrade());
    }
}

#[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
pub(crate) mod otlp {
    //! The OTLP exporters.

    use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig as _};

    use crate::Error;
    use crate::config::{OtlpConfig, Protocol};

    fn failed(e: impl std::fmt::Display) -> Error {
        Error::Exporter(e.to_string())
    }

    /// The HTTP endpoint of one signal: the base plus `/v1/traces` or
    /// `/v1/logs`.
    #[cfg(feature = "otlp-http")]
    fn signal(endpoint: &str, path: &str) -> String {
        format!("{}{path}", endpoint.trim_end_matches('/'))
    }

    #[cfg(feature = "otlp-grpc")]
    fn runtime() -> Result<(), Error> {
        // tonic's channel spawns its worker on the runtime it is built in.
        tokio::runtime::Handle::try_current()
            .map(drop)
            .map_err(|_| Error::NoRuntime)
    }

    pub(crate) fn spans(config: &OtlpConfig) -> Result<SpanExporter, Error> {
        match config.protocol {
            #[cfg(feature = "otlp-grpc")]
            Protocol::Grpc => {
                runtime()?;
                SpanExporter::builder()
                    .with_tonic()
                    .with_endpoint(config.endpoint.clone())
                    .with_timeout(config.timeout)
                    .build()
                    .map_err(failed)
            }
            #[cfg(feature = "otlp-http")]
            Protocol::Http => SpanExporter::builder()
                .with_http()
                .with_endpoint(signal(&config.endpoint, "/v1/traces"))
                .with_timeout(config.timeout)
                .build()
                .map_err(failed),
            #[allow(unreachable_patterns)]
            other => Err(Error::Unsupported(other)),
        }
    }

    pub(crate) fn logs(config: &OtlpConfig) -> Result<LogExporter, Error> {
        match config.protocol {
            #[cfg(feature = "otlp-grpc")]
            Protocol::Grpc => {
                runtime()?;
                LogExporter::builder()
                    .with_tonic()
                    .with_endpoint(config.endpoint.clone())
                    .with_timeout(config.timeout)
                    .build()
                    .map_err(failed)
            }
            #[cfg(feature = "otlp-http")]
            Protocol::Http => LogExporter::builder()
                .with_http()
                .with_endpoint(signal(&config.endpoint, "/v1/logs"))
                .with_timeout(config.timeout)
                .build()
                .map_err(failed),
            #[allow(unreachable_patterns)]
            other => Err(Error::Unsupported(other)),
        }
    }
}
