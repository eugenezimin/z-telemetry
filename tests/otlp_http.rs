//! The same over OTLP/HTTP: protobuf bodies posted to `/v1/traces` and
//! `/v1/logs` under the configured endpoint.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message as _;
use z_telemetry::{ColorMode, OtlpConfig, Protocol, Service, TelemetryConfig};

type Posts = Arc<Mutex<Vec<(String, Bytes)>>>;

async fn receive(
    posts: Posts,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let path = request.uri().path().to_owned();
    let body = request.into_body().collect().await?.to_bytes();
    posts.lock().unwrap().push((path, body));
    Ok(Response::builder()
        .header("content-type", "application/x-protobuf")
        .body(Full::new(Bytes::new()))
        .unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spans_and_logs_are_posted() {
    let posts = Posts::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = {
        let posts = posts.clone();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let posts = posts.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |r| receive(posts.clone(), r));
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tcp), service)
                        .await;
                });
            }
        })
    };

    let mut config = TelemetryConfig::default();
    config.console.color = ColorMode::Never;
    let mut otlp = OtlpConfig::new(format!("http://{addr}/"));
    otlp.protocol = Protocol::Http;
    config.otlp = Some(otlp);
    let telemetry = z_telemetry::init(&config, Service::new("billing", "0.3.0")).unwrap();
    {
        let span = tracing::info_span!("invoice");
        let _entered = span.enter();
        tracing::warn!(amount = 10, "invoice overdue");
    }
    tokio::task::spawn_blocking(move || telemetry.shutdown())
        .await
        .unwrap();

    let posts = posts.lock().unwrap().clone();
    let traces: Vec<_> = posts.iter().filter(|(p, _)| p == "/v1/traces").collect();
    let logs: Vec<_> = posts.iter().filter(|(p, _)| p == "/v1/logs").collect();
    assert!(
        !traces.is_empty() && !logs.is_empty(),
        "{:?}",
        posts.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    let spans = ExportTraceServiceRequest::decode(traces[0].1.clone()).unwrap();
    let span_names: Vec<_> = spans
        .resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .flat_map(|ss| &ss.spans)
        .map(|s| s.name.clone())
        .collect();
    assert!(span_names.contains(&"invoice".to_owned()), "{span_names:?}");
    let records = ExportLogsServiceRequest::decode(logs[0].1.clone()).unwrap();
    let severities: Vec<_> = records
        .resource_logs
        .iter()
        .flat_map(|rl| &rl.scope_logs)
        .flat_map(|sl| &sl.log_records)
        .map(|r| r.severity_text.clone())
        .collect();
    assert!(severities.contains(&"WARN".to_owned()), "{severities:?}");
    server.abort();
}
