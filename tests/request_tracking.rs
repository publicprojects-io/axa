//! `axa-tracing` behind the standard middleware stack.

#![cfg(feature = "tracing")]

use axa::{
    config::Http,
    middleware,
    tracing::{current_request_id, track},
};
use axum::{
    Router,
    body::Body,
    http::{Request, Response},
    middleware::from_fn,
    routing::get,
};
use tower::ServiceExt;

fn config() -> Http {
    Http {
        body_limit: 1024,
        compression: false,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cors_origins: Vec::new(),
        hsts: false,
        hsts_max_age: 0,
        csp: None,
    }
}

async fn serve(request: Request<Body>) -> Response<Body> {
    let router = Router::new()
        .route(
            "/",
            get(|| async { current_request_id().unwrap_or_default() }),
        )
        .layer(from_fn(track));
    middleware::apply(router, &config())
        .unwrap()
        .oneshot(request)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_tracked_id_is_the_id_the_response_carries() {
    // A minted id is the case that can differ, as nothing supplied it.
    for caller in [None, Some("abc-123"), Some("abc<>123"), Some("<>")] {
        let mut request = Request::get("/");
        if let Some(caller) = caller {
            request = request.header("x-request-id", caller);
        }
        let response = serve(request.body(Body::empty()).unwrap()).await;
        let header = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body, header.as_bytes(), "{caller:?}");
    }
}
