# z-telemetry

Telemetry for services, from the `tracing` events they already emit:

- **Console logs** people can read: one coloured line per event on stdout with local (or UTC)
  time, level, `module:line`, the message and its data, and the trace id when exporting.
- **OpenTelemetry**: the same spans and events exported as traces and logs over OTLP (gRPC or
  HTTP) to a Collector, for Grafana's stack (Loki, Tempo) or any other backend.
- **Trace propagation**: W3C `traceparent` in and out of HTTP and gRPC headers, and through event
  headers, so one request is one trace across services.

> **Status: in development (0.1.0).**
