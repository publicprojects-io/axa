//! OTLP export of traces and request metrics for Axum services.
//!
//! A service already has spans, from `tracing`. This crate sends them to an
//! `OpenTelemetry` collector over OTLP/gRPC, and adds the request metric and the
//! `traceparent` response header.
//!
//! Export is off until an endpoint is set, so a service sends nothing anywhere
//! unless someone chose where it goes.
//!
//! | Variable | Meaning |
//! |---|---|
//! | `OTLP_ENDPOINT` | Collector URL, `https` unless loopback. Unset or blank means off |
//! | `OTLP_SERVICE_NAME` | Replaces the name the service passes in |
//! | `OTLP_ALLOW_INSECURE` | `true`, `1`, `yes`, or `on` allows plain `http` to a remote collector |
//!
//! `OTEL_RESOURCE_ATTRIBUTES` works as usual, for example for `service.instance.id`.
//!
//! # Use
//!
//! Start it inside a Tokio runtime, with more than one thread, because a flush blocks
//! its thread. The collector need not be up: nothing connects until the first export.
//!
//! ```no_run
//! use axa_opentelemetry::{Config, DIAGNOSTIC_DIRECTIVES, middleware, start};
//! use axum::{Router, middleware::from_fn_with_state, routing::get};
//!
//! # async fn example() -> Result<(), axa_opentelemetry::Error> {
//! let config = Config::from_env("my-service", env!("CARGO_PKG_VERSION"));
//! let telemetry = start(&config)?;
//! // Then, with `axa`:
//! // axa::logger::init_with("my_service", &logger, telemetry.layers(), DIAGNOSTIC_DIRECTIVES);
//! # let _ = DIAGNOSTIC_DIRECTIVES;
//!
//! // Add this before `axa::middleware::apply`, so that it runs inside the request span.
//! let app: Router = Router::new()
//!     .route("/", get(|| async { "ok" }))
//!     .layer(from_fn_with_state(telemetry.http_metrics(), middleware));
//! # let _ = app;
//!
//! // At the end of graceful shutdown, or the last few seconds of spans are lost.
//! telemetry.shutdown();
//! # Ok(())
//! # }
//! ```
//!
//! # What leaves the service
//!
//! Every span the log filter lets through, with its fields, which is why plain `http`
//! to a remote collector is refused. Treat a span field like a log field. `axa`'s
//! request span carries the route's template and never the path, so an identifier in
//! a path does not reach the collector by that route.
//!
//! The metric's attributes are the method, status, and route template, each from a
//! small set, so that a caller cannot grow the number of series.
//!
//! # What this crate leaves out
//!
//! It does not continue a trace from an incoming `traceparent`, so each service's
//! trace starts at its own edge. Use `axa_tracing::propagate` to follow a request
//! across services by its id. There is no OTLP log signal, only traces and metrics.
//!
//! An event logged inside a span still travels with that span as a span event, so log
//! text reaches the collector. Treat the collector as you treat the logs.

use std::time::{Duration, Instant};

use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, Uri, uri::Scheme},
    middleware::Next,
    response::Response,
};
use opentelemetry::{
    KeyValue, global,
    metrics::Histogram,
    trace::{Status, TraceContextExt, TracerProvider as _},
};
use opentelemetry_otlp::{
    MetricExporter, SpanExporter, WithExportConfig, WithTonicConfig,
    tonic_types::transport::ClientTlsConfig,
};
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider, trace::SdkTracerProvider};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{Layer, Registry};

/// A layer ready for `axa::logger::init_with`.
pub type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Pass these to `axa::logger::init_with`, or a failing export is invisible: the
/// default log filter names only known modules, and these are not among them.
pub const DIAGNOSTIC_DIRECTIVES: &[&str] = &[
    "opentelemetry=warn",
    "opentelemetry_sdk=warn",
    "opentelemetry-otlp=warn",
];

/// How long one export may take.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

/// The response header carrying the request's trace, so that an operator can go from
/// a response straight to its trace.
const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");

/// The route of a request that matched none.
const UNMATCHED: &str = "unmatched";

/// Why telemetry could not start.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The endpoint is not a URL with a host.
    #[error("OTLP endpoint is not a valid URL")]
    InvalidEndpoint,

    /// The endpoint would send spans in the clear to another host.
    #[error("OTLP endpoint must use https unless it is loopback or OTLP_ALLOW_INSECURE is set")]
    InsecureEndpoint,

    /// An exporter could not be built.
    #[error("OTLP exporter: {0}")]
    Exporter(#[from] opentelemetry_otlp::ExporterBuildError),
}

/// What to export, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The `service.name` of everything exported.
    pub service_name: String,
    /// The `service.version` of everything exported.
    pub service_version: String,
    /// The collector's URL. `None` means export is off.
    pub endpoint: Option<String>,
    /// Allow plain `http` to a host other than this one.
    pub allow_insecure: bool,
}

impl Config {
    /// A configuration with export off.
    #[must_use]
    pub fn new(service_name: impl Into<String>, service_version: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: service_version.into(),
            endpoint: None,
            allow_insecure: false,
        }
    }

    /// Read the `OTLP_*` variables from the process environment.
    #[must_use]
    pub fn from_env(service_name: &str, service_version: &str) -> Self {
        Self::from_lookup(service_name, service_version, &|name| {
            std::env::var(name).ok()
        })
    }

    /// Read the `OTLP_*` variables through `lookup`. A blank value counts as unset.
    #[must_use]
    pub fn from_lookup(
        service_name: &str,
        service_version: &str,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let value = |name| {
            lookup(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        Self {
            service_name: value("OTLP_SERVICE_NAME").unwrap_or_else(|| service_name.to_string()),
            service_version: service_version.to_string(),
            endpoint: value("OTLP_ENDPOINT"),
            allow_insecure: value("OTLP_ALLOW_INSECURE").is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "true" | "1" | "yes" | "on"
                )
            }),
        }
    }

    fn resource(&self) -> Resource {
        Resource::builder()
            .with_service_name(self.service_name.clone())
            .with_attribute(KeyValue::new(
                "service.version",
                self.service_version.clone(),
            ))
            .build()
    }
}

/// Check an endpoint, and say whether it uses TLS.
fn is_https(endpoint: &str, allow_insecure: bool) -> Result<bool, Error> {
    let uri: Uri = endpoint.parse().map_err(|_| Error::InvalidEndpoint)?;
    let host = uri.host().ok_or(Error::InvalidEndpoint)?;
    match uri.scheme() {
        Some(scheme) if *scheme == Scheme::HTTPS => Ok(true),
        Some(scheme) if *scheme == Scheme::HTTP => {
            if allow_insecure || is_loopback(host) {
                Ok(false)
            } else {
                Err(Error::InsecureEndpoint)
            }
        }
        _ => Err(Error::InvalidEndpoint),
    }
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The roots to trust. The exporter's own default on `https` trusts none, so every
/// export would fail at the handshake.
fn tls() -> ClientTlsConfig {
    ClientTlsConfig::new().with_enabled_roots()
}

fn tracer_provider(
    config: &Config,
    endpoint: &str,
    https: bool,
) -> Result<SdkTracerProvider, Error> {
    let mut builder = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(EXPORT_TIMEOUT);
    if https {
        builder = builder.with_tls_config(tls());
    }
    Ok(SdkTracerProvider::builder()
        .with_resource(config.resource())
        .with_batch_exporter(builder.build()?)
        .build())
}

fn meter_provider(config: &Config, endpoint: &str, https: bool) -> Result<SdkMeterProvider, Error> {
    let mut builder = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(EXPORT_TIMEOUT);
    if https {
        builder = builder.with_tls_config(tls());
    }
    Ok(SdkMeterProvider::builder()
        .with_resource(config.resource())
        .with_periodic_exporter(builder.build()?)
        .build())
}

/// Telemetry that has started, or that is off.
#[derive(Debug)]
pub struct Telemetry {
    service_name: String,
    providers: Option<(SdkTracerProvider, SdkMeterProvider)>,
}

/// Start exporting, if `config` has an endpoint. Must run inside a Tokio runtime.
///
/// # Errors
///
/// When the endpoint is invalid, or insecure and not allowed, or an exporter cannot
/// be built. A collector that is down is not an error: spans wait in a bounded queue,
/// and are dropped when it fills, so a request never waits for a collector.
pub fn start(config: &Config) -> Result<Telemetry, Error> {
    let Some(endpoint) = config.endpoint.as_deref() else {
        return Ok(Telemetry {
            service_name: config.service_name.clone(),
            providers: None,
        });
    };
    let https = is_https(endpoint, config.allow_insecure)?;
    let tracers = tracer_provider(config, endpoint, https)?;
    let meters = meter_provider(config, endpoint, https)?;
    global::set_tracer_provider(tracers.clone());
    global::set_meter_provider(meters.clone());
    Ok(Telemetry {
        service_name: config.service_name.clone(),
        providers: Some((tracers, meters)),
    })
}

impl Telemetry {
    /// Whether export is running.
    #[must_use]
    pub const fn exporting(&self) -> bool {
        self.providers.is_some()
    }

    /// The layers that turn spans into exports, for `axa::logger::init_with`. Empty
    /// when export is off.
    #[must_use]
    pub fn layers(&self) -> Vec<BoxedLayer> {
        self.providers
            .iter()
            .map(|(tracers, _)| {
                tracing_opentelemetry::layer()
                    .with_tracer(tracers.tracer(self.service_name.clone()))
                    .boxed()
            })
            .collect()
    }

    /// The state for [`middleware`]. It records nothing when export is off.
    #[must_use]
    pub fn http_metrics(&self) -> HttpMetrics {
        let meter = match &self.providers {
            Some((_, meters)) => {
                opentelemetry::metrics::MeterProvider::meter(meters, "axa-opentelemetry")
            }
            None => global::meter("axa-opentelemetry"),
        };
        HttpMetrics {
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_description("Duration of inbound HTTP requests.")
                .with_unit("s")
                .build(),
        }
    }

    /// Send what is queued now.
    pub fn flush(&self) {
        if let Some((tracers, meters)) = &self.providers {
            if let Err(error) = tracers.force_flush() {
                tracing::warn!(%error, "trace_flush_failed");
            }
            if let Err(error) = meters.force_flush() {
                tracing::warn!(%error, "metric_flush_failed");
            }
        }
    }

    /// Send what is queued, and stop. Call it once, at the end of graceful shutdown,
    /// from a thread that may block.
    pub fn shutdown(&self) {
        if let Some((tracers, meters)) = &self.providers {
            if let Err(error) = tracers.shutdown() {
                tracing::warn!(%error, "trace_shutdown_failed");
            }
            if let Err(error) = meters.shutdown() {
                tracing::warn!(%error, "metric_shutdown_failed");
            }
        }
    }
}

/// The instruments [`middleware`] records to.
#[derive(Debug, Clone)]
pub struct HttpMetrics {
    duration: Histogram<f64>,
}

/// A request's method for a metric. Any other name becomes `_OTHER`, as the
/// `OpenTelemetry` conventions say, so that a caller cannot create series at will.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::CONNECT => "CONNECT",
        Method::OPTIONS => "OPTIONS",
        Method::TRACE => "TRACE",
        Method::PATCH => "PATCH",
        _ => "_OTHER",
    }
}

/// Middleware recording `http.server.request.duration`, adding the route and status
/// to the current span, and putting `traceparent` on the response.
///
/// It opens no span of its own, and uses the one `axa` opens for each request. Add it
/// inside that span, on the router before `axa::middleware::apply`.
pub async fn middleware(
    State(metrics): State<HttpMetrics>,
    request: Request,
    next: Next,
) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED, MatchedPath::as_str)
        .to_string();
    let span = tracing::Span::current();
    let mut recording = Recording {
        metrics,
        method: method_label(request.method()),
        route,
        started: Instant::now(),
        span: span.clone(),
        done: false,
    };

    let mut response = next.run(request).await;

    recording.finish(response.status());
    if let Some(value) = traceparent(&span) {
        response.headers_mut().insert(TRACEPARENT, value);
    }
    response
}

/// One request being measured.
///
/// A panic unwinds through [`middleware`], and a timeout or a client that hangs up
/// drops it, so neither reaches the line after `next.run`. Both still run this `Drop`,
/// so those requests are counted, with `error.type` saying which it was, and with no
/// status, because the outer layers choose that.
struct Recording {
    metrics: HttpMetrics,
    method: &'static str,
    route: String,
    started: Instant,
    span: tracing::Span,
    done: bool,
}

impl Recording {
    fn finish(&mut self, status: StatusCode) {
        self.done = true;
        let code = i64::from(status.as_u16());
        self.span.set_attribute("http.route", self.route.clone());
        self.span.set_attribute("http.response.status_code", code);
        if status.is_server_error() {
            self.span.set_status(Status::error("server error"));
        }
        self.record(KeyValue::new("http.response.status_code", code));
    }

    fn record(&self, outcome: KeyValue) {
        self.metrics.duration.record(
            self.started.elapsed().as_secs_f64(),
            &[
                KeyValue::new("http.request.method", self.method),
                KeyValue::new("http.route", self.route.clone()),
                outcome,
            ],
        );
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let error = if std::thread::panicking() {
            "panic"
        } else {
            "cancelled"
        };
        self.span.set_attribute("http.route", self.route.clone());
        self.span.set_attribute("error.type", error);
        self.span.set_status(Status::error(error));
        self.record(KeyValue::new("error.type", error));
    }
}

/// The W3C `traceparent` of `span`. `None` without export, as the span has no trace.
fn traceparent(span: &tracing::Span) -> Option<HeaderValue> {
    let context = span.context();
    let trace = context.span();
    let trace = trace.span_context();
    trace
        .is_valid()
        .then(|| {
            format!(
                "00-{}-{}-{:02x}",
                trace.trace_id(),
                trace.span_id(),
                trace.trace_flags().to_u8()
            )
        })
        .and_then(|value| HeaderValue::from_str(&value).ok())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        move |name| pairs.get(name).cloned()
    }

    #[test]
    fn export_is_off_until_an_endpoint_is_set() {
        assert_eq!(Config::new("a", "1").endpoint, None);
        assert_eq!(Config::from_lookup("a", "1", &lookup(&[])).endpoint, None);
        let blank = lookup(&[("OTLP_ENDPOINT", "  ")]);
        assert_eq!(Config::from_lookup("a", "1", &blank).endpoint, None);
    }

    #[test]
    fn the_environment_sets_the_endpoint_and_replaces_the_name() {
        let vars = lookup(&[
            ("OTLP_ENDPOINT", " https://collector.example:4317 "),
            ("OTLP_SERVICE_NAME", "renamed"),
            ("OTLP_ALLOW_INSECURE", "YES"),
        ]);
        let config = Config::from_lookup("a", "1", &vars);
        assert_eq!(
            config.endpoint.as_deref(),
            Some("https://collector.example:4317")
        );
        assert_eq!(config.service_name, "renamed");
        assert_eq!(config.service_version, "1");
        assert!(config.allow_insecure);
    }

    #[test]
    fn anything_but_a_clear_yes_keeps_insecure_refused() {
        for value in ["", "false", "0", "no", "off", "maybe"] {
            let vars = lookup(&[("OTLP_ALLOW_INSECURE", value)]);
            assert!(
                !Config::from_lookup("a", "1", &vars).allow_insecure,
                "{value}"
            );
        }
    }

    #[test]
    fn https_is_accepted_anywhere() {
        assert!(is_https("https://collector.example:4317", false).unwrap());
        assert!(is_https("https://localhost:4317", false).unwrap());
    }

    #[test]
    fn plain_http_is_for_loopback() {
        for endpoint in [
            "http://localhost:4317",
            "http://LOCALHOST:4317",
            "http://127.0.0.1:4317",
            "http://127.1.2.3:4317",
            "http://[::1]:4317",
        ] {
            assert!(!is_https(endpoint, false).unwrap(), "{endpoint}");
        }
    }

    #[test]
    fn plain_http_to_another_host_is_refused() {
        for endpoint in [
            "http://collector.example:4317",
            "http://10.0.0.1:4317",
            "http://localhost.example:4317",
            "http://localhost@collector.example:4317",
        ] {
            assert!(
                matches!(is_https(endpoint, false), Err(Error::InsecureEndpoint)),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn plain_http_to_another_host_can_be_allowed() {
        assert!(!is_https("http://collector.internal:4317", true).unwrap());
    }

    #[test]
    fn a_bad_endpoint_is_refused() {
        for endpoint in ["", "not a url", "collector:4317", "ftp://collector:4317"] {
            assert!(
                matches!(is_https(endpoint, true), Err(Error::InvalidEndpoint)),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn an_error_never_quotes_the_endpoint() {
        let error = is_https("http://user:secret@collector.example", false).unwrap_err();
        assert!(!error.to_string().contains("secret"), "{error}");
    }

    #[test]
    fn the_diagnostic_directives_are_valid() {
        for directive in DIAGNOSTIC_DIRECTIVES {
            assert!(
                tracing_subscriber::EnvFilter::try_new(directive).is_ok(),
                "{directive}"
            );
        }
    }

    #[test]
    fn an_unknown_method_cannot_create_a_series() {
        let made_up = Method::from_bytes(b"MADEUP").unwrap();
        assert_eq!(method_label(&made_up), "_OTHER");
        assert_eq!(method_label(&Method::GET), "GET");
    }
}
