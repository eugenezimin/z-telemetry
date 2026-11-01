//! The configuration: plain serde types a service embeds in its own
//! configuration (a YAML section, env overrides through its loader).
//! Unknown keys are refused; every key has a default except
//! `otlp.endpoint`.
//!
//! ```yaml
//! console:
//!   level: info,my_service=debug   # tracing filter; RUST_LOG wins
//!   color: auto                    # auto | always | never
//!   time: local                    # local | utc
//! otlp:                            # absent: nothing is exported
//!   endpoint: http://127.0.0.1:4317
//!   protocol: grpc                 # grpc | http
//!   level: info                    # events exported as logs
//!   sample_ratio: 1.0              # 0.0 to 1.0, at the root of each trace
//!   timeout: 10s
//! environment: development
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

/// Everything z-telemetry needs.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct TelemetryConfig {
    /// Logs on stdout.
    pub console: ConsoleConfig,
    /// OpenTelemetry export; `None` exports nothing.
    pub otlp: Option<OtlpConfig>,
    /// `deployment.environment.name` on everything exported
    /// (`development`, `staging`, `production`, …).
    pub environment: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            console: ConsoleConfig::default(),
            otlp: None,
            environment: "development".into(),
        }
    }
}

/// Logs on stdout.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConsoleConfig {
    /// What is printed: a `tracing` filter such as `info,my_service=debug`
    /// (levels `trace`, `debug`, `info`, `warn`, `error`, per module).
    /// The `RUST_LOG` variable wins over it.
    pub level: String,
    /// Colours: `auto` (only on a terminal, and not when `NO_COLOR` is set),
    /// `always` or `never`.
    pub color: ColorMode,
    /// Timestamps in `local` time or `utc`.
    pub time: TimeZone,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            color: ColorMode::Auto,
            time: TimeZone::Local,
        }
    }
}

/// When the console uses colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// Only when stdout is a terminal and `NO_COLOR` is not set.
    #[default]
    Auto,
    /// Always (e.g. a log viewer that renders ANSI colours).
    Always,
    /// Never.
    Never,
}

/// The time zone of console timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeZone {
    /// The host's local time.
    #[default]
    Local,
    /// UTC.
    Utc,
}

/// OpenTelemetry export over OTLP: spans as traces, events as logs.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OtlpConfig {
    /// The Collector: `http://host:4317` for gRPC, `http://host:4318` for
    /// HTTP (the signal paths `/v1/traces`, `/v1/logs` are added).
    pub endpoint: String,
    /// `grpc` or `http` (protobuf over HTTP).
    #[serde(default)]
    pub protocol: Protocol,
    /// Which events are exported as logs: a `tracing` filter, like
    /// `console.level`. Spans are exported when sampled.
    #[serde(default = "OtlpConfig::default_level")]
    pub level: String,
    /// The share of new traces recorded, from `0.0` to `1.0`; a trace
    /// continued from a caller follows the caller's decision.
    #[serde(default = "OtlpConfig::default_sample_ratio")]
    pub sample_ratio: f64,
    /// Longest export request.
    #[serde(with = "humantime_serde", default = "OtlpConfig::default_timeout")]
    pub timeout: Duration,
}

impl OtlpConfig {
    fn default_level() -> String {
        "info".into()
    }

    const fn default_sample_ratio() -> f64 {
        1.0
    }

    const fn default_timeout() -> Duration {
        Duration::from_secs(10)
    }

    /// An export to `endpoint` with every other setting at its default.
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            protocol: Protocol::default(),
            level: Self::default_level(),
            sample_ratio: Self::default_sample_ratio(),
            timeout: Self::default_timeout(),
        }
    }
}

/// The OTLP transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// gRPC (the Collector's port 4317).
    #[default]
    Grpc,
    /// Protobuf over HTTP (the Collector's port 4318).
    Http,
}

/// Settings that cannot be used, one problem per entry, each with its path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .0.join("\n"))]
pub struct ConfigError(pub Vec<String>);

impl TelemetryConfig {
    /// Check every setting, reporting all problems at once.
    ///
    /// # Errors
    /// [`ConfigError`] with one line per problem.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut problems = Vec::new();
        if let Err(e) = EnvFilter::builder().parse(&self.console.level) {
            problems.push(format!("console.level: {e}"));
        }
        if self.environment.trim().is_empty() {
            problems.push("environment: must not be empty".to_owned());
        }
        if let Some(otlp) = &self.otlp {
            if !(otlp.endpoint.starts_with("http://") || otlp.endpoint.starts_with("https://")) {
                problems
                    .push("otlp.endpoint: must be http://host:port or https://host:port".into());
            }
            if let Err(e) = EnvFilter::builder().parse(&otlp.level) {
                problems.push(format!("otlp.level: {e}"));
            }
            if !(0.0..=1.0).contains(&otlp.sample_ratio) {
                problems.push("otlp.sample_ratio: must be between 0.0 and 1.0".into());
            }
            if otlp.timeout.is_zero() {
                problems.push("otlp.timeout: must be positive".into());
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ConfigError(problems))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> Result<TelemetryConfig, serde_saphyr::Error> {
        serde_saphyr::from_str(text)
    }

    #[test]
    fn everything_has_a_default() {
        let c = yaml("{}").unwrap();
        assert_eq!(c, TelemetryConfig::default());
        assert_eq!(c.console.level, "info");
        assert_eq!(
            (c.console.color, c.console.time),
            (ColorMode::Auto, TimeZone::Local)
        );
        assert!(c.otlp.is_none());
        c.validate().unwrap();
    }

    #[test]
    fn every_option_parses() {
        let c = yaml(
            "console: { level: 'info,my_service=debug', color: never, time: utc }\n\
             otlp: { endpoint: 'http://127.0.0.1:4318', protocol: http, level: warn, \
                     sample_ratio: 0.25, timeout: 3s }\n\
             environment: production",
        )
        .unwrap();
        assert_eq!(c.console.color, ColorMode::Never);
        assert_eq!(c.console.time, TimeZone::Utc);
        let otlp = c.otlp.as_ref().unwrap();
        assert_eq!(otlp.protocol, Protocol::Http);
        assert_eq!(otlp.timeout, Duration::from_secs(3));
        assert_eq!(c.environment, "production");
        c.validate().unwrap();
        // Only the endpoint is required.
        let c = yaml("otlp: { endpoint: 'http://collector:4317' }").unwrap();
        assert_eq!(c.otlp, Some(OtlpConfig::new("http://collector:4317")));
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(yaml("console: { colour: never }").is_err());
        assert!(yaml("log_format: json").is_err());
        assert!(yaml("otlp: { endpoint: 'http://x:1', sampling: 1 }").is_err());
        assert!(yaml("console: { color: rainbow }").is_err());
    }

    #[test]
    fn every_problem_is_reported() {
        let mut c = TelemetryConfig::default();
        c.console.level = "info,=bogus[".into();
        c.environment = " ".into();
        c.otlp = Some(OtlpConfig {
            endpoint: "collector:4317".into(),
            level: "nope[".into(),
            sample_ratio: 2.0,
            timeout: Duration::ZERO,
            protocol: Protocol::Grpc,
        });
        let problems = c.validate().unwrap_err().0;
        assert_eq!(problems.len(), 6, "{problems:#?}");
        assert!(problems.iter().all(|p| p.contains(':')));
    }
}
