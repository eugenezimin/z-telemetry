# z-telemetry

Telemetry for services, from the `tracing` events they already emit:

- **Console logs** people can read: one coloured line per event on stdout with local (or UTC)
  time, level and `module:line` in brackets, the message in the level's colour, then its data and
  the trace id when exporting in `META::[…]`.
- **OpenTelemetry**: the same spans and events exported as traces and logs over OTLP (gRPC or
  HTTP) to a Collector, for Grafana's stack (Loki, Tempo) or any other backend.
- **Trace propagation**: W3C `traceparent` in and out of HTTP and gRPC headers, and through event
  headers, so one request is one trace across services.

> **Status: 0.2.0.** Console, OTLP export (gRPC and HTTP), propagation.

## Usage

```rust,ignore
// At startup, inside the tokio runtime (needed for OTLP over gRPC):
let config: z_telemetry::TelemetryConfig = /* a section of the service's configuration */;
let telemetry = z_telemetry::init(&config, z_telemetry::Service::new("users", env!("CARGO_PKG_VERSION")))?;

// Incoming requests join the caller's trace:
let span = tracing::info_span!("http", method = %req.method(), path = %req.uri().path());
z_telemetry::propagation::set_parent(&span, req.headers());

// At the end, flush what is still buffered:
telemetry.shutdown();
```

A line on the console:

```text
[2026-11-01 16:49:58.025][INFO][users_service::serve:412] serving  META::[ http=0.0.0.0:8080 trace=4bf92f35]
```

Try it: `cargo run --example console`.

## Features

| Feature | Default | What |
|---|---|---|
| `otlp-grpc` | yes | OTLP over gRPC (tonic). `init` must run inside a tokio runtime. |
| `otlp-http` | yes | OTLP over HTTP with protobuf bodies. |

## Documentation

- [Configuration](docs/configuration.md): every setting, the console line, what is exported, propagation.
- [`z-telemetry.example.yaml`](z-telemetry.example.yaml): every setting with its default.

## With Grafana

Point `otlp.endpoint` at a Collector or at Grafana's all-in-one `grafana/otel-lgtm` image
(OTLP on `:4317` and `:4318`, Grafana on `:3000`):
- logs land in Loki, labelled with `service_name`;
- traces land in Tempo;
- a log line's trace id leads to its trace.

## License

MIT or Apache-2.0, at your option.
