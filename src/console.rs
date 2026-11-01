//! The console format: one readable line per event.
//!
//! ```text
//! 2026-11-01 16:49:58.025  INFO my_service::serve:412  serving  http=0.0.0.0:8080 request_id=7f3c… trace=4bf92f35
//! ```
//!
//! Time (local or UTC, milliseconds), the level (padded, coloured), the
//! event's `module::path:line`, the message, then the event's own fields,
//! then the fields of the spans it happened in (outermost first), then the
//! short trace id when spans are exported. Field names are dimmed; values
//! are quoted when they contain spaces, so a line splits unambiguously.

use std::fmt::{self, Write as _};
use std::io::IsTerminal as _;

use nu_ansi_term::{Color, Style};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields};
use tracing_subscriber::registry::{LookupSpan, SpanRef};

use crate::config::{ColorMode, TimeZone};
use crate::otel::TraceIds;

/// Whether the console uses colours under `mode`: `auto` colours only a
/// terminal, and never when `NO_COLOR` is set (<https://no-color.org>).
#[must_use]
pub fn colors(mode: ColorMode) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
                && std::io::stdout().is_terminal()
        }
    }
}

/// The event format.
#[derive(Debug, Clone)]
pub struct ConsoleFormat {
    time: TimeZone,
    trace_ids: Option<TraceIds>,
}

impl ConsoleFormat {
    /// Timestamps in `time`.
    #[must_use]
    pub const fn new(time: TimeZone) -> Self {
        Self {
            time,
            trace_ids: None,
        }
    }

    /// Print the short trace id of the span an event happened in.
    #[must_use]
    pub(crate) fn with_trace_ids(mut self, trace_ids: TraceIds) -> Self {
        self.trace_ids = Some(trace_ids);
        self
    }
}

fn level_style(level: Level) -> Style {
    match level {
        Level::TRACE => Color::DarkGray.normal(),
        Level::DEBUG => Color::Blue.normal(),
        Level::INFO => Color::Green.normal(),
        Level::WARN => Color::Yellow.normal(),
        Level::ERROR => Color::Red.bold(),
    }
}

/// `style` when colouring, plain otherwise.
fn paint(ansi: bool, style: Style, text: &str) -> String {
    if ansi {
        style.paint(text).to_string()
    } else {
        text.to_owned()
    }
}

/// A value as printed: quoted (with escapes) when it would not read as one
/// word.
fn value(raw: &str) -> String {
    if raw.is_empty()
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '=')
    {
        format!("{raw:?}")
    } else {
        raw.to_owned()
    }
}

/// Collects `key=value` pairs (and the message, for events).
struct Fields<'a> {
    ansi: bool,
    message: Option<String>,
    out: &'a mut String,
}

impl Fields<'_> {
    fn pair(&mut self, name: &str, raw: &str) {
        if name == "message" && self.message.is_none() {
            self.message = Some(raw.to_owned());
            return;
        }
        // `log` records forwarded into tracing carry these; the line has them already.
        if name.starts_with("log.") {
            return;
        }
        let _ = write!(
            self.out,
            " {}{}",
            paint(self.ansi, Style::new().dimmed(), &format!("{name}=")),
            value(raw)
        );
    }
}

impl Visit for Fields<'_> {
    fn record_str(&mut self, field: &Field, v: &str) {
        self.pair(field.name(), v);
    }

    fn record_error(&mut self, field: &Field, v: &(dyn std::error::Error + 'static)) {
        self.pair(field.name(), &v.to_string());
    }

    fn record_debug(&mut self, field: &Field, v: &dyn fmt::Debug) {
        self.pair(field.name(), &format!("{v:?}"));
    }
}

/// Formats span fields as ` key=value` pairs, stored with each span.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsoleFields;

impl<'writer> FormatFields<'writer> for ConsoleFields {
    fn format_fields<R: RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut out = String::new();
        let mut visitor = Fields {
            ansi: writer.has_ansi_escapes(),
            message: None,
            out: &mut out,
        };
        fields.record(&mut visitor);
        if let Some(message) = visitor.message.take() {
            visitor.pair("message", &message);
        }
        writer.write_str(&out)
    }
}

impl<S, N> FormatEvent<S, N> for ConsoleFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'w> FormatFields<'w> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let ansi = writer.has_ansi_escapes();
        let meta = event.metadata();
        let time = match self.time {
            TimeZone::Local => chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
            TimeZone::Utc => chrono::Utc::now()
                .format("%Y-%m-%d %H:%M:%S%.3fZ")
                .to_string(),
        };
        let level = *meta.level();
        let place = match (meta.module_path(), meta.line()) {
            (Some(module), Some(line)) => format!("{module}:{line}"),
            (Some(module), None) => module.to_owned(),
            (None, _) => meta.target().to_owned(),
        };

        let mut fields = String::new();
        let mut visitor = Fields {
            ansi,
            message: None,
            out: &mut fields,
        };
        event.record(&mut visitor);
        let message = visitor.message.unwrap_or_default();

        write!(
            writer,
            "{} {} {}  {}",
            paint(ansi, Style::new().dimmed(), &time),
            paint(ansi, level_style(level), &format!("{:>5}", level.as_str())),
            paint(ansi, Style::new().dimmed(), &place),
            message
        )?;
        if !fields.is_empty() {
            write!(writer, " {fields}")?;
        }
        // The spans the event happened in, outermost first.
        let mut trace = None;
        if let Some(scope) = ctx.event_scope() {
            let spans: Vec<SpanRef<'_, S>> = scope.from_root().collect();
            let mut first = true;
            for span in &spans {
                let ext = span.extensions();
                if let Some(stored) = ext.get::<FormattedFields<N>>()
                    && !stored.is_empty()
                {
                    if first {
                        writer.write_char(' ')?;
                        first = false;
                    }
                    write!(writer, "{stored}")?;
                }
            }
            if let (Some(ids), Some(innermost)) = (&self.trace_ids, spans.last()) {
                trace = ids.of(&innermost.id());
            }
        }
        if let Some(id) = trace {
            write!(
                writer,
                "  {}{}",
                paint(ansi, Style::new().dimmed(), "trace="),
                id.get(..8).unwrap_or(&id)
            )?;
        }
        writeln!(writer)
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;
    use crate::testing::Captured;

    /// The lines `f` logs, at `time`, with or without colours.
    fn lines(time: TimeZone, ansi: bool, f: impl FnOnce()) -> Vec<String> {
        let out = Captured::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .event_format(ConsoleFormat::new(time))
                .fmt_fields(ConsoleFields)
                .with_ansi(ansi)
                .with_writer(out.clone()),
        );
        tracing::subscriber::with_default(subscriber, f);
        out.text().lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_line_has_time_level_place_message_and_data() {
        let line = lines(TimeZone::Utc, false, || {
            tracing::info!(port = 8080, host = "0.0.0.0", "serving");
        });
        let [line] = line.as_slice() else {
            panic!("{line:?}")
        };
        // 2026-11-01 16:49:58.025Z  INFO z_telemetry::console::tests:NNN  serving  port=8080 host=0.0.0.0
        let (time, rest) = line.split_once("Z ").unwrap();
        assert_eq!(time.len(), "2026-11-01 16:49:58.025".len(), "{line}");
        assert!(
            rest.starts_with(" INFO z_telemetry::console::tests:"),
            "{line}"
        );
        assert!(
            rest.ends_with("  serving  port=8080 host=0.0.0.0"),
            "{line}"
        );
    }

    #[test]
    fn local_time_has_milliseconds() {
        let line = lines(TimeZone::Local, false, || tracing::warn!("careful"));
        let time: String = line[0]
            .chars()
            .take("2026-11-01 16:49:58.025".len())
            .collect();
        assert!(
            chrono::NaiveDateTime::parse_from_str(&time, "%Y-%m-%d %H:%M:%S%.3f").is_ok(),
            "{time}"
        );
        assert!(line[0].contains(" WARN "));
    }

    #[test]
    fn values_with_spaces_are_quoted_and_errors_print() {
        let error = std::io::Error::other("disk full");
        let line = lines(TimeZone::Utc, false, || {
            tracing::error!(
                error = &error as &dyn std::error::Error,
                path = "a b",
                "write failed"
            );
        });
        assert!(
            line[0].ends_with(r#"write failed  error="disk full" path="a b""#),
            "{}",
            line[0]
        );
        assert!(line[0].contains("ERROR"));
    }

    #[test]
    fn span_fields_follow_outermost_first() {
        let line = lines(TimeZone::Utc, false, || {
            let outer = tracing::info_span!("http", request_id = "r1");
            let _o = outer.enter();
            let inner = tracing::info_span!("job", run = 7);
            let _i = inner.enter();
            tracing::debug!(step = "merge", "working");
        });
        assert!(
            line[0].ends_with("  working  step=merge  request_id=r1 run=7"),
            "{}",
            line[0]
        );
    }

    #[test]
    fn colours_are_ansi_and_only_when_asked() {
        let plain = lines(TimeZone::Utc, false, || tracing::error!(k = 1, "boom"));
        assert!(!plain[0].contains('\u{1b}'));
        let coloured = lines(TimeZone::Utc, true, || tracing::error!(k = 1, "boom"));
        assert!(
            coloured[0].contains("\u{1b}[1;31mERROR"),
            "{:?}",
            coloured[0]
        );
        assert!(
            coloured[0].contains("\u{1b}[2mk=\u{1b}[0m1"),
            "{:?}",
            coloured[0]
        );
        assert!(!colors(ColorMode::Never));
        assert!(colors(ColorMode::Always));
    }
}
