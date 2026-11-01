//! The console format at every level, with fields and spans:
//!
//! ```sh
//! cargo run --example console                               # defaults
//! cargo run --example console -- z-telemetry.example.yaml   # a configuration file
//! cargo run --example console | cat                         # piped: no colours
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use z_telemetry::{Service, TelemetryConfig};

fn main() {
    let config: TelemetryConfig = match std::env::args().nth(1) {
        Some(path) => serde_saphyr::from_str(&std::fs::read_to_string(path).unwrap()).unwrap(),
        None => TelemetryConfig {
            console: z_telemetry::ConsoleConfig {
                level: "trace".into(),
                ..Default::default()
            },
            ..Default::default()
        },
    };
    let telemetry = z_telemetry::init(&config, Service::new("example", "0.1.0")).unwrap();

    tracing::info!(addr = "0.0.0.0:8080", "serving");
    let request = tracing::info_span!(
        "http",
        method = "POST",
        path = "/orders",
        request_id = "7f3c2a"
    );
    {
        let _entered = request.enter();
        tracing::trace!(bytes = 512, "body read");
        tracing::debug!(customer = "c-17", items = 3, "validating the order");
        tracing::info!(order = 42, total = "99.90 EUR", "order placed");
        let job = tracing::info_span!("payment", provider = "acme");
        let _job = job.enter();
        tracing::warn!(attempt = 2, "payment provider slow");
        let error = std::io::Error::other("connection reset by peer");
        tracing::error!(error = &error as &dyn std::error::Error, "payment failed");
    }
    telemetry.shutdown();
}
