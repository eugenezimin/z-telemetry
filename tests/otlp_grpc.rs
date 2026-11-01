//! Spans and events reach an OTLP gRPC receiver (a Collector stand-in),
//! with the service's resource attributes, when telemetry shuts down.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{
    LogsService, LogsServiceServer,
};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{
    TraceService, TraceServiceServer,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::resource::v1::Resource;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};
use z_telemetry::{ColorMode, OtlpConfig, Service, TelemetryConfig};

#[derive(Clone, Default)]
struct Received {
    spans: Arc<Mutex<Vec<ExportTraceServiceRequest>>>,
    logs: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
}

#[tonic::async_trait]
impl TraceService for Received {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        self.spans.lock().unwrap().push(request.into_inner());
        Ok(Response::new(ExportTraceServiceResponse::default()))
    }
}

#[tonic::async_trait]
impl LogsService for Received {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        self.logs.lock().unwrap().push(request.into_inner());
        Ok(Response::new(ExportLogsServiceResponse::default()))
    }
}

fn attribute(resource: Option<&Resource>, key: &str) -> Option<String> {
    resource?
        .attributes
        .iter()
        .find(|kv| kv.key == key)
        .and_then(|kv| match kv.value.as_ref()?.value.as_ref()? {
            Value::StringValue(s) => Some(s.clone()),
            _ => None,
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spans_and_logs_reach_the_collector() {
    let received = Received::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(TraceServiceServer::new(received.clone()))
            .add_service(LogsServiceServer::new(received.clone()))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );

    let mut config = TelemetryConfig::default();
    config.console.color = ColorMode::Never;
    config.environment = "test".into();
    config.otlp = Some(OtlpConfig::new(format!("http://{addr}")));
    let telemetry = z_telemetry::init(&config, Service::new("orders", "2.0.1")).unwrap();
    {
        let span = tracing::info_span!("http", path = "/orders");
        let _entered = span.enter();
        tracing::info!(order = 42, "order placed");
    }
    // Shutting down flushes; it blocks, so off the async workers.
    tokio::task::spawn_blocking(move || telemetry.shutdown())
        .await
        .unwrap();

    let spans = received.spans.lock().unwrap().clone();
    let resource_spans: Vec<_> = spans.iter().flat_map(|r| &r.resource_spans).collect();
    let span = resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .flat_map(|ss| &ss.spans)
        .find(|s| s.name == "http")
        .expect("the span was exported");
    let resource = resource_spans[0].resource.as_ref();
    assert_eq!(
        attribute(resource, "service.name").as_deref(),
        Some("orders")
    );
    assert_eq!(
        attribute(resource, "service.version").as_deref(),
        Some("2.0.1")
    );
    assert_eq!(
        attribute(resource, "deployment.environment.name").as_deref(),
        Some("test")
    );
    assert!(attribute(resource, "service.instance.id").is_some());

    let logs = received.logs.lock().unwrap().clone();
    let record = logs
        .iter()
        .flat_map(|r| &r.resource_logs)
        .flat_map(|rl| &rl.scope_logs)
        .flat_map(|sl| &sl.log_records)
        .find(|r| {
            matches!(r.body.as_ref().and_then(|b| b.value.as_ref()),
                Some(Value::StringValue(s)) if s == "order placed")
        })
        .expect("the event was exported as a log");
    assert_eq!(
        record.trace_id, span.trace_id,
        "the log carries its span's trace"
    );
    assert_eq!(record.severity_text, "INFO");
    server.abort();
}
