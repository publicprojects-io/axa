//! Limits behind the standard middleware stack, as a service mounts them.

#![cfg(feature = "ratelimit")]

use std::net::SocketAddr;

use axa::{config::Http, middleware, ratelimit};
use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header::HeaderName},
    middleware::from_fn_with_state,
    response::Response,
    routing::get,
};
use tower::ServiceExt;

fn http(trusted_proxies: &[&str]) -> Http {
    Http {
        body_limit: 1024,
        compression: false,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cors_origins: Vec::new(),
        hsts: false,
        hsts_max_age: 0,
        csp: None,
        trusted_proxies: trusted_proxies
            .iter()
            .map(|net| net.parse().unwrap())
            .collect(),
    }
}

/// One request allowed per client, inside the stack.
fn app(trusted_proxies: &[&str]) -> Router {
    let limiter = ratelimit::Limiter::new(1.try_into().unwrap(), 60.try_into().unwrap());
    let routes = Router::new()
        .route("/", get(|| async { "ok" }))
        .layer(from_fn_with_state(limiter, ratelimit::by_client));
    middleware::apply(routes, &http(trusted_proxies)).unwrap()
}

async fn call(app: &Router, peer: &str, forwarded: Option<&str>) -> Response {
    let mut request = Request::get("/");
    if let Some(forwarded) = forwarded {
        request = request.header("x-forwarded-for", forwarded);
    }
    let mut request = request.body(Body::empty()).unwrap();
    let peer: SocketAddr = format!("{peer}:50000").parse().unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn a_forged_header_does_not_buy_a_fresh_limit() {
    let app = app(&["10.0.0.0/8"]);
    let first = call(&app, "10.0.0.1", Some("198.51.100.1, 203.0.113.9")).await;
    assert_eq!(first.status(), StatusCode::OK);
    // The same client behind the proxy, inventing a new first entry.
    let forged = call(&app, "10.0.0.1", Some("198.51.100.2, 203.0.113.9")).await;
    assert_eq!(forged.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_header_from_an_untrusted_peer_is_ignored() {
    let app = app(&[]);
    let first = call(&app, "203.0.113.9", Some("198.51.100.1")).await;
    assert_eq!(first.status(), StatusCode::OK);
    let forged = call(&app, "203.0.113.9", Some("198.51.100.2")).await;
    assert_eq!(forged.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn clients_behind_one_proxy_get_a_limit_each() {
    let app = app(&["10.0.0.0/8"]);
    for client in ["203.0.113.9", "203.0.113.10"] {
        let response = call(&app, "10.0.0.1", Some(client)).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn a_refusal_gets_the_standard_headers() {
    let app = app(&[]);
    call(&app, "203.0.113.9", None).await;
    let refused = call(&app, "203.0.113.9", None).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    for header in ["x-request-id", "x-content-type-options"] {
        assert!(
            refused
                .headers()
                .contains_key(HeaderName::from_static(header)),
            "{header}"
        );
    }
}
