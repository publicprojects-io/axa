//! `axa-tera` pages under the production stack.
#![cfg(feature = "tera")]

use axa::{
    config::{DEFAULT_CSP, Http},
    middleware,
    tera::{PAGE_CSP, Templates},
};
use axum::{
    Router,
    body::Body,
    http::{Request, header},
    routing::get,
};
use tower::ServiceExt;

fn production_http() -> Http {
    Http {
        body_limit: 1_000,
        compression: false,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cors_origins: Vec::new(),
        hsts: true,
        hsts_max_age: 31_536_000,
        csp: Some(DEFAULT_CSP.to_string()),
    }
}

async fn policy_of(router: Router) -> String {
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let response = middleware::apply(router, &production_http())
        .unwrap()
        .oneshot(request)
        .await
        .unwrap();
    response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_page_can_load_its_own_files_in_production() {
    let router = Router::new().route(
        "/",
        get(|| async {
            let templates = Templates::from_raw([("a.html", "<p>hi</p>")]).unwrap();
            templates.html("a.html", &serde_json::json!({})).unwrap()
        }),
    );
    assert_eq!(policy_of(router).await, PAGE_CSP);
    assert!(PAGE_CSP.contains("default-src 'self'"));
}

#[tokio::test]
async fn json_keeps_the_strict_policy() {
    let router = Router::new().route("/", get(|| async { "{}" }));
    assert_eq!(policy_of(router).await, DEFAULT_CSP);
}
