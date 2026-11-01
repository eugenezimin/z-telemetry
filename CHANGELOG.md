# Changelog

## [Unreleased]

## [0.1.0]

### Added
- `TelemetryConfig`: `console` (`level`, `color`, `time`), optional `otlp` (`endpoint`, `protocol`,
  `level`, `sample_ratio`, `timeout`) and `environment`. Plain serde types, unknown keys refused,
  every problem reported at once.
- The console format: one line per event with local or UTC time (milliseconds), the coloured
  level, `module::path:line`, the message, the event's and its spans' fields as `key=value`, and
  the short trace id when exporting. Colours follow the terminal and `NO_COLOR`. Written through a
  non-blocking stdout.
- `init`: the global subscriber with the console and, when configured, OTLP export of spans as
  traces and events as logs (gRPC or HTTP, batched, parent-based ratio sampling, resource
  attributes `service.name`, `service.version`, `service.instance.id`,
  `deployment.environment.name`). `log` records are forwarded. The `Telemetry` guard flushes on
  shutdown or drop.
- `propagation`: `set_parent`, `set_parent_from`, `inject` and `traceparent` for W3C trace
  context in HTTP and gRPC headers and in event headers.
