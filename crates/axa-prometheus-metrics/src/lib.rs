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

use axum::{
    http::{StatusCode, header::CONTENT_TYPE as CONTENT_TYPE_HEADER},
    response::{IntoResponse, Response},
};
pub use prometheus::{
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounter,
    IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
};
use prometheus::{Encoder, TextEncoder};

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
