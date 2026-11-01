//! W3C trace context across services: continue the caller's trace from its
//! `traceparent` (HTTP and gRPC headers, or an event's headers), and hand
//! the current trace on to the next call.
//!
//! These use the propagator [`init`](crate::init) installs; without it they
//! do nothing, so code calling them runs unchanged in tests.
//!
//! ```
//! # fn handle(request: &http::Request<()>) {
//! let span = tracing::info_span!("http", path = %request.uri().path());
//! // Before the span is entered: the request joins the caller's trace.
//! z_telemetry::propagation::set_parent(&span, request.headers());
//! # }
//! ```

use std::collections::HashMap;

use http::HeaderMap;
use http::header::{HeaderName, HeaderValue};
use opentelemetry::propagation::{Extractor, Injector};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

struct Headers<'a>(&'a HeaderMap);

impl Extractor for Headers<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(HeaderName::as_str).collect()
    }
}

struct HeadersMut<'a>(&'a mut HeaderMap);

impl Injector for HeadersMut<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            self.0.insert(name, value);
        }
    }
}

/// Make `span` a child of the trace in `headers` (`traceparent`,
/// `tracestate`). Call it right after creating the span, before it is
/// entered; without a valid `traceparent` the span starts a new trace.
pub fn set_parent(span: &Span, headers: &HeaderMap) {
    let context = opentelemetry::global::get_text_map_propagator(|p| p.extract(&Headers(headers)));
    // Fails only when the span was started already; it then keeps its own trace.
    let _ = span.set_parent(context);
}

/// Make `span` a child of the trace in a `traceparent` value carried
/// elsewhere, such as an event's headers.
pub fn set_parent_from(span: &Span, traceparent: &str) {
    let carrier = HashMap::from([("traceparent".to_owned(), traceparent.to_owned())]);
    let context = opentelemetry::global::get_text_map_propagator(|p| p.extract(&carrier));
    let _ = span.set_parent(context);
}

/// Put `span`'s trace context into `headers` for an outgoing call.
pub fn inject(span: &Span, headers: &mut HeaderMap) {
    let context = span.context();
    opentelemetry::global::get_text_map_propagator(|p| {
        p.inject_context(&context, &mut HeadersMut(headers));
    });
}

/// `span`'s `traceparent`, to carry in an event's headers; `None` when it
/// has no trace (telemetry not exporting, or not started).
#[must_use]
pub fn traceparent(span: &Span) -> Option<String> {
    let context = span.context();
    let mut carrier = HashMap::new();
    opentelemetry::global::get_text_map_propagator(|p| p.inject_context(&context, &mut carrier));
    carrier.remove("traceparent")
}
