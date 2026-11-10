//! Prometheus text exposition.
//!
//! A service keeps its own [`Registry`] and domain counters, and uses this
//! crate to serve them at a scrape path.
//!
//! ```
//! use axa_prometheus_metrics::{IntCounter, Opts, Registry, render};
//!
//! let registry = Registry::new();
//! let jobs = IntCounter::with_opts(Opts::new("jobs_total", "Jobs run.")).unwrap();
//! registry.register(Box::new(jobs.clone())).unwrap();
//! jobs.inc();
//! assert!(render(&registry).contains("jobs_total 1"));
//! ```
//!
//! Mount [`DEFAULT_PATH`] at the root, outside `/api`, and keep it public so a
//! scraper needs no bearer token. Metric names and labels are then readable by
//! anyone who can reach the port, so they must never carry personal data.
//!
//! [`RequestMetrics`] measures every request by its route template, never its path:
//!
//! ```
//! use axa_prometheus_metrics::{Registry, RequestMetrics, record};
//! use axum::{Router, middleware::from_fn_with_state, routing::get};
//!
//! let registry = Registry::new();
//! let metrics = RequestMetrics::register(&registry).unwrap();
//! let router: Router = Router::new()
//!     .route("/places/{id}", get(|| async { "ok" }))
//!     .layer(from_fn_with_state(metrics, record));
//! # let _ = router;
//! ```
//!
//! Add the layer to the router before `axa::middleware::apply`, so that it sits inside
//! routing and sees the route template.

use axum::{
    extract::{MatchedPath, Request, State},
    http::{Method, StatusCode, header::CONTENT_TYPE as CONTENT_TYPE_HEADER},
    middleware::Next,
    response::{IntoResponse, Response},
};
pub use prometheus::{
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounter,
    IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
};
use prometheus::{Encoder, TextEncoder};
use std::time::Instant;

/// The route label for a request that matched no route, so that a scanner's paths
/// share one label rather than making one each.
pub const UNMATCHED: &str = "unmatched";

/// Request durations, by method, route template, and status code.
#[derive(Clone, Debug)]
pub struct RequestMetrics {
    duration: HistogramVec,
}

impl RequestMetrics {
    /// Make the histogram `http_server_request_duration_seconds` in `registry`, labelled
    /// `method`, `route`, and `status`. Its `_count` is the number of requests.
    ///
    /// # Errors
    ///
    /// Fails when `registry` already holds a metric of that name.
    pub fn register(registry: &Registry) -> prometheus::Result<Self> {
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "http_server_request_duration_seconds",
                "How long requests took to answer, by method, route template, and status.",
            ),
            &["method", "route", "status"],
        )?;
        registry.register(Box::new(duration.clone()))?;
        Ok(Self { duration })
    }

    /// Record one request.
    fn observe(&self, method: &Method, route: &str, status: StatusCode, seconds: f64) {
        self.duration
            .with_label_values(&[method_label(method), route, status.as_str()])
            .observe(seconds);
    }
}

/// A method's label: one of the standard methods, or `other`, so that a client cannot
/// make a label by sending a method of its own.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        _ => "other",
    }
}

/// Record how long each request took, for `axum::middleware::from_fn_with_state`. The
/// route is the template the request matched, or [`UNMATCHED`].
pub async fn record(
    State(metrics): State<RequestMetrics>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| UNMATCHED.to_string(), |route| route.as_str().to_string());
    let started = Instant::now();
    let response = next.run(request).await;
    metrics.observe(
        &method,
        &route,
        response.status(),
        started.elapsed().as_secs_f64(),
    );
    response
}

/// The Prometheus text exposition media type.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4";

/// The scrape path the family's services use.
pub const DEFAULT_PATH: &str = "/metrics.prom";

/// Render every metric in `registry`.
///
/// Encoding failure gives a comment line rather than a panic, because a bad
/// metric must not take a scrape or a service down with it.
#[must_use]
pub fn render(registry: &Registry) -> String {
    let mut buffer = Vec::new();
    if let Err(error) = TextEncoder::new().encode(&registry.gather(), &mut buffer) {
        return format!("# metrics encoding failed: {error}\n");
    }
    String::from_utf8(buffer).unwrap_or_default()
}

/// A `200` carrying rendered metrics with the exposition media type.
#[must_use]
pub fn response(body: String) -> Response {
    (StatusCode::OK, [(CONTENT_TYPE_HEADER, CONTENT_TYPE)], body).into_response()
}

/// Render `registry` as a scrape response.
#[must_use]
pub fn scrape(registry: &Registry) -> Response {
    response(render(registry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request, routing::get};
    use tower::ServiceExt;

    fn registry() -> (Registry, IntCounter) {
        let registry = Registry::new();
        let counter = IntCounter::with_opts(Opts::new("jobs_total", "Jobs run.")).unwrap();
        registry.register(Box::new(counter.clone())).unwrap();
        (registry, counter)
    }

    async fn call(router: &Router, method: &str, path: &str) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let _ = router.clone().oneshot(request).await.unwrap();
    }

    #[tokio::test]
    async fn requests_are_measured_by_route_template_not_path() {
        let registry = Registry::new();
        let metrics = RequestMetrics::register(&registry).unwrap();
        let router = Router::new()
            .route("/places/{id}", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(metrics, record));
        call(&router, "GET", "/places/1").await;
        call(&router, "GET", "/places/2").await;
        call(&router, "GET", "/wp-admin/one").await;
        call(&router, "GET", "/wp-admin/two").await;
        call(&router, "BREW", "/places/3").await;
        let text = render(&registry);
        let count = |labels: &str| {
            let line = format!("http_server_request_duration_seconds_count{{{labels}}}");
            text.lines()
                .find(|row| row.starts_with(&line))
                .unwrap_or_else(|| panic!("no {line} in\n{text}"))
                .rsplit(' ')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            count(r#"method="GET",route="/places/{id}",status="200""#),
            "2"
        );
        // A scanner's paths share one label.
        assert_eq!(count(r#"method="GET",route="unmatched",status="404""#), "2");
        // So does a method a client made up.
        assert_eq!(
            count(r#"method="other",route="/places/{id}",status="405""#),
            "1"
        );
        assert!(!text.contains("wp-admin"), "{text}");
        assert!(!text.contains("/places/1"), "{text}");
    }

    #[test]
    fn registering_twice_in_one_registry_fails() {
        let registry = Registry::new();
        assert!(RequestMetrics::register(&registry).is_ok());
        assert!(RequestMetrics::register(&registry).is_err());
    }

    #[test]
    fn an_empty_registry_renders_nothing() {
        assert_eq!(render(&Registry::new()), "");
    }

    #[test]
    fn counters_render_in_the_exposition_format() {
        let (registry, counter) = registry();
        counter.inc_by(3);
        let text = render(&registry);
        assert!(text.contains("# HELP jobs_total Jobs run."), "{text}");
        assert!(text.contains("# TYPE jobs_total counter"), "{text}");
        assert!(text.contains("jobs_total 3"), "{text}");
    }

    #[test]
    fn labelled_counters_keep_their_labels() {
        let registry = Registry::new();
        let counter = IntCounterVec::new(Opts::new("hits_total", "Hits."), &["route"]).unwrap();
        registry.register(Box::new(counter.clone())).unwrap();
        counter.with_label_values(&["home"]).inc();
        assert!(render(&registry).contains("hits_total{route=\"home\"} 1"));
    }

    #[tokio::test]
    async fn the_response_uses_the_exposition_media_type() {
        let response = response("x 1\n".to_string());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE_HEADER], CONTENT_TYPE);
    }

    #[tokio::test]
    async fn a_live_scrape_reflects_later_increments() {
        let (registry, counter) = registry();
        let app = Router::new().route(
            DEFAULT_PATH,
            get(move || {
                let registry = registry.clone();
                async move { scrape(&registry) }
            }),
        );
        counter.inc();
        let request = Request::builder()
            .uri(DEFAULT_PATH)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("jobs_total 1"));
    }
}
