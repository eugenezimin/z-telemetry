//! Telemetry for services, from the `tracing` events they already emit:
//!
//! - **console**: one readable, coloured line per event on stdout: local (or
//!   UTC) time, level, `module:line`, the message and its data;
//! - **OpenTelemetry**: the same spans and events exported as traces and
//!   logs over OTLP, to a Collector (Grafana's stack, or any other);
//! - **propagation**: W3C trace context in and out of HTTP and gRPC
//!   headers, and through event headers.

pub mod config;
pub mod console;

pub use config::{
    ColorMode, ConfigError, ConsoleConfig, OtlpConfig, Protocol, TelemetryConfig, TimeZone,
};
