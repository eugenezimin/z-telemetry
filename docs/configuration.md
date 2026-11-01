# Configuration

z-telemetry reads one `TelemetryConfig`, a plain serde type a service embeds in its own
configuration (a YAML section, overridden by environment variables through the service's loader).
Unknown keys are refused. `validate` (and `init`) report every problem at once, one per line, with
the setting's path.

The complete example is [`z-telemetry.example.yaml`](../z-telemetry.example.yaml).

## `console`: logs on stdout

| Field | Default | Meaning |
|---|---|---|
| `level` | `info` | What is printed: a `tracing` filter such as `info,my_service=debug` (levels `trace`, `debug`, `info`, `warn`, `error`, per module or target). The `RUST_LOG` environment variable wins over it. |
| `color` | `auto` | `auto`: colours only when stdout is a terminal, and never when `NO_COLOR` is set. `always` or `never` force it. |
| `time` | `local` | Timestamps in `local` time or `utc`, with milliseconds. |

One line per event:

```text
2026-11-01 16:49:58.025  INFO my_service::orders:88  order placed  order=42 total="99.90 EUR"  method=POST path=/orders request_id=7f3c2a  trace=4bf92f35
```

- the time;
- the level, coloured: TRACE grey, DEBUG blue, INFO green, WARN yellow, ERROR bold red;
- the event's `module::path:line`;
- the message;
- the event's own fields;
- the fields of the spans it happened in, outermost first;
- when exporting, the first 8 hex digits of the trace id.

Field names are dimmed. Values are quoted when they contain spaces, quotes or `=`.

Output goes through a non-blocking writer, so a slow terminal or pipe never stalls async workers.
`Telemetry::shutdown` flushes it.

## `otlp`: OpenTelemetry export (optional)

Absent, nothing is exported.

| Field | Default | Meaning |
|---|---|---|
| `endpoint` | — (required) | The Collector, `http://host:port`: `4317` for `grpc`, `4318` for `http` (the signal paths `/v1/traces` and `/v1/logs` are added). TLS to the Collector is not supported yet. |
| `protocol` | `grpc` | `grpc` (feature `otlp-grpc`; `init` must run inside a tokio runtime) or `http` (protobuf, feature `otlp-http`). |
| `level` | `info` | Which events are exported as logs, a `tracing` filter. |
| `sample_ratio` | `1.0` | The share of new traces recorded, `0.0`–`1.0`. A trace continued from a caller follows the caller's decision (parent-based sampling). |
| `timeout` | `10s` | Longest export request. |

What is exported:
- **spans** as traces, at the console's level;
- **events** as logs at `otlp.level`, each with the trace and span it happened in;
- on everything, the resource `service.name`, `service.version`, `service.instance.id` (one per
  process) and `deployment.environment.name`.

Exports are batched on the SDK's own threads. The exporters' own crates (`opentelemetry*`,
`tonic`, `h2`, `hyper`, `reqwest`, `tower`) are never exported, so exporting cannot feed itself.
Metrics are not exported; keep the service's Prometheus endpoint and let the Collector scrape it.

## `environment`

`deployment.environment.name` on everything exported (`development`, `staging`, `production`, …).
Default `development`.

## Propagation

`z_telemetry::propagation` carries W3C trace context:

| Function | Use |
|---|---|
| `set_parent(&span, &headers)` | On an incoming HTTP or gRPC request, right after creating its span: the request joins the caller's trace. |
| `set_parent_from(&span, traceparent)` | For work started by a message, such as an event whose headers carry `traceparent`. |
| `inject(&span, &mut headers)` | On an outgoing call: the callee joins this trace. |
| `traceparent(&span)` | The value to put in an event's headers. |

They use the propagator `init` installs, and do nothing without it.
