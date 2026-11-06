//! Export against a real OTLP/gRPC collector in this process, so that the assertions
//! are about the protobuf that crossed a socket.

use std::{
    fmt::Write,
    future::Future,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axa_opentelemetry::{Config, Error, Telemetry, middleware, start};
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
    routing::get,
};
use opentelemetry_proto::tonic::{
    collector::{
        metrics::v1::{
            ExportMetricsServiceRequest, ExportMetricsServiceResponse,
            metrics_service_server::{MetricsService, MetricsServiceServer},
        },
        trace::v1::{
            ExportTraceServiceRequest, ExportTraceServiceResponse,
            trace_service_server::{TraceService, TraceServiceServer},
        },
    },
    common::v1::{AnyValue, KeyValue, any_value::Value},
    metrics::v1::metric::Data,
};
use tokio::net::TcpListener;
use tower::ServiceExt;
use tracing::Instrument;
use tracing_subscriber::{Registry, layer::SubscriberExt};

/// Everything the collector has received.
#[derive(Clone, Default)]
struct Collector {
    traces: Arc<Mutex<Vec<ExportTraceServiceRequest>>>,
    metrics: Arc<Mutex<Vec<ExportMetricsServiceRequest>>>,
}

#[tonic::async_trait]
impl TraceService for Collector {
    async fn export(
        &self,
        request: tonic::Request<ExportTraceServiceRequest>,
    ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
        self.traces.lock().unwrap().push(request.into_inner());
        Ok(tonic::Response::new(ExportTraceServiceResponse::default()))
    }
}

#[tonic::async_trait]
impl MetricsService for Collector {
    async fn export(
        &self,
        request: tonic::Request<ExportMetricsServiceRequest>,
    ) -> Result<tonic::Response<ExportMetricsServiceResponse>, tonic::Status> {
        self.metrics.lock().unwrap().push(request.into_inner());
        Ok(tonic::Response::new(ExportMetricsServiceResponse::default()))
    }
}

async fn collector() -> (String, Collector) {
    let listener = TcpListener::bind::<SocketAddr>("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let collector = Collector::default();
    let service = collector.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(TraceServiceServer::new(service.clone()))
            .add_service(MetricsServiceServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .ok();
    });
    (format!("http://{address}"), collector)
}

fn config(endpoint: Option<String>) -> Config {
    Config {
        endpoint,
        ..Config::new("test-service", "1.2.3")
    }
}

fn text(value: Option<&AnyValue>) -> Option<&str> {
    match value?.value.as_ref()? {
        Value::StringValue(value) => Some(value),
        _ => None,
    }
}

fn integer(value: Option<&AnyValue>) -> Option<i64> {
    match value?.value.as_ref()? {
        Value::IntValue(value) => Some(*value),
        _ => None,
    }
}

fn attribute<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a AnyValue> {
    attributes
        .iter()
        .find(|pair| pair.key == key)
        .and_then(|pair| pair.value.as_ref())
}

async fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    done()
}

/// Stands in for `axa`'s request span, which this crate cannot depend on.
async fn request_span(request: Request, next: Next) -> Response {
    let span = tracing::info_span!("http-request", request_id = "abc");
    next.run(request).instrument(span).await
}

fn app(telemetry: &Telemetry) -> Router {
    Router::new()
        .route("/things/{id}", get(|| async { "ok" }))
        .route(
            "/broken",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .layer(from_fn_with_state(telemetry.http_metrics(), middleware))
        .layer(from_fn(request_span))
}

async fn with_subscriber<F: Future>(telemetry: &Telemetry, future: F) -> F::Output {
    let subscriber = Registry::default().with(telemetry.layers());
    let _guard = tracing::subscriber::set_default(subscriber);
    future.await
}

async fn get_path(app: Router, path: &str) -> Response {
    let request = axum::http::Request::get(path).body(Body::empty()).unwrap();
    app.oneshot(request).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_reaches_the_collector_as_a_span_and_a_metric() {
    let (endpoint, collector) = collector().await;
    let telemetry = start(&config(Some(endpoint))).unwrap();
    assert!(telemetry.exporting());

    let response =
        with_subscriber(&telemetry, get_path(app(&telemetry), "/things/secret-id")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let header = response
        .headers()
        .get("traceparent")
        .expect("traceparent header")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(header.len(), 55, "{header}");
    assert!(header.starts_with("00-"), "{header}");

    telemetry.flush();
    let received = {
        let collector = collector.clone();
        wait_for(move || {
            !collector.traces.lock().unwrap().is_empty()
                && !collector.metrics.lock().unwrap().is_empty()
        })
        .await
    };
    assert!(received, "nothing reached the collector");

    // The span carries the service, the template, the status, and its own fields.
    let traces = collector.traces.lock().unwrap();
    let resource_spans = &traces[0].resource_spans[0];
    let resource = resource_spans.resource.as_ref().unwrap();
    assert_eq!(
        text(attribute(&resource.attributes, "service.name")),
        Some("test-service")
    );
    assert_eq!(
        text(attribute(&resource.attributes, "service.version")),
        Some("1.2.3")
    );
    let span = &resource_spans.scope_spans[0].spans[0];
    assert_eq!(span.name, "http-request");
    assert_eq!(
        text(attribute(&span.attributes, "http.route")),
        Some("/things/{id}")
    );
    assert_eq!(
        integer(attribute(&span.attributes, "http.response.status_code")),
        Some(200)
    );
    assert_eq!(text(attribute(&span.attributes, "request_id")), Some("abc"));
    assert!(
        header.contains(&hex(&span.trace_id)),
        "{header} is not the exported trace"
    );

    // The metric uses the template, so that the path's id goes nowhere.
    let metrics = collector.metrics.lock().unwrap();
    let metric = metrics
        .iter()
        .flat_map(|request| &request.resource_metrics)
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics)
        .find(|metric| metric.name == "http.server.request.duration")
        .expect("request metric");
    let Some(Data::Histogram(histogram)) = &metric.data else {
        panic!("not a histogram");
    };
    let point = &histogram.data_points[0];
    assert_eq!(point.count, 1);
    assert_eq!(
        text(attribute(&point.attributes, "http.route")),
        Some("/things/{id}")
    );
    assert_eq!(
        text(attribute(&point.attributes, "http.request.method")),
        Some("GET")
    );
    assert!(!format!("{metrics:?}").contains("secret-id"));
    drop((traces, metrics));
    telemetry.shutdown();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").unwrap();
        text
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_error_marks_the_span() {
    let (endpoint, collector) = collector().await;
    let telemetry = start(&config(Some(endpoint))).unwrap();

    with_subscriber(&telemetry, get_path(app(&telemetry), "/broken")).await;
    telemetry.flush();
    let received = {
        let collector = collector.clone();
        wait_for(move || !collector.traces.lock().unwrap().is_empty()).await
    };
    assert!(received, "nothing reached the collector");

    let traces = collector.traces.lock().unwrap();
    let span = &traces[0].resource_spans[0].scope_spans[0].spans[0];
    let status = span.status.as_ref().expect("status");
    assert_eq!(status.code, 2, "{status:?}");
    drop(traces);
    telemetry.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn off_means_no_layers_no_header_and_the_request_still_works() {
    let telemetry = start(&config(None)).unwrap();
    assert!(!telemetry.exporting());
    assert!(telemetry.layers().is_empty());

    let response = with_subscriber(&telemetry, get_path(app(&telemetry), "/things/1")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("traceparent").is_none());

    telemetry.flush();
    telemetry.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_collector_that_is_down_stops_nothing() {
    // Bound and released, so that nothing listens here.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let telemetry = start(&config(Some(format!("http://{address}")))).unwrap();
    let started = Instant::now();
    let response = with_subscriber(&telemetry, get_path(app(&telemetry), "/things/1")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(started.elapsed() < Duration::from_secs(1));
    telemetry.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_https_endpoint_builds_with_roots_to_trust() {
    let telemetry = start(&config(Some("https://collector.example:4317".to_string()))).unwrap();
    assert!(telemetry.exporting());
}

#[tokio::test(flavor = "multi_thread")]
async fn start_refuses_cleartext_to_another_host() {
    let result = start(&config(Some("http://collector.example:4317".to_string())));
    assert!(matches!(result, Err(Error::InsecureEndpoint)));
}
