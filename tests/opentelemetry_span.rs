//! `axa-opentelemetry` behind the standard middleware stack.
//!
//! This has its own test binary, so its own process, because the subscriber it
//! installs is process-wide.

#![cfg(feature = "opentelemetry")]

use axa::{
    config::Http,
    middleware,
    opentelemetry::{Config, middleware as telemetry_middleware, start},
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    middleware::from_fn_with_state,
    routing::get,
};
use tower::ServiceExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::test(flavor = "multi_thread")]
async fn the_middleware_joins_the_request_span_axa_opens() {
    // Nothing listens here, and nothing needs to: the trace is local until exported.
    let config = Config {
        endpoint: Some("http://127.0.0.1:1".to_string()),
        ..Config::new("test-service", "1.0.0")
    };
    let telemetry = start(&config).unwrap();
    tracing_subscriber::registry()
        .with(telemetry.layers())
        .init();

    let router = Router::new()
        .route("/things/{id}", get(|| async { "ok" }))
        .layer(from_fn_with_state(
            telemetry.http_metrics(),
            telemetry_middleware,
        ));
    let http = Http {
        body_limit: 1024,
        compression: false,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cross_site_paths: Vec::new(),
        cors_origins: Vec::new(),
        hsts: false,
        hsts_max_age: 0,
        csp: None,
        permissions_policy: None,
        trusted_proxies: Vec::new(),
    };
    let response = middleware::apply(router, &http)
        .unwrap()
        .oneshot(Request::get("/things/1").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // A traceparent exists only if there was a request span for this middleware to
    // join, as it opens none of its own.
    let traceparent = response.headers()["traceparent"].to_str().unwrap();
    assert_eq!(traceparent.len(), 55, "{traceparent}");
}
