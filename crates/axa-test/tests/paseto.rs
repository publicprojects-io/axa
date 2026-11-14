//! Test tokens through the real PASETO guard, and the isolation check on a route that
//! takes the tenant from them.

#![cfg(feature = "paseto")]

use std::sync::Arc;

use axa_paseto_auth::{Guard, GuardConfig, Tenant, middleware};
use axa_test::{
    TestServer,
    isolation::{Probe, assert_hidden},
    paseto::TestKeys,
};
use axum::{Router, extract::Path, http::StatusCode, middleware::from_fn_with_state, routing::get};

const ISSUER: &str = "authentication-service";
const AUDIENCE: &str = "todo-service";

fn server(keys: &TestKeys) -> TestServer {
    let guard = Guard::new(
        GuardConfig::new("todo"),
        Arc::new(authentication_verifier::ReloadableVerifier::new(
            keys.verifier(ISSUER, AUDIENCE),
        )),
        Arc::new(authentication_verifier::ReloadablePolicy::new(
            authentication_verifier::Policy::default_policy(),
        )),
    );
    let router = Router::new()
        // Todo 1 is acme's.
        .route(
            "/api/todos/{id}",
            get(|tenant: Tenant, Path(id): Path<u32>| async move {
                if id == 1 && tenant.as_str() == "acme" {
                    (StatusCode::OK, "Acme's plan")
                } else {
                    (StatusCode::NOT_FOUND, "")
                }
            }),
        )
        .route(
            "/api/todos",
            get(|tenant: Tenant| async move {
                if tenant.as_str() == "acme" {
                    "[\"Acme's plan\"]"
                } else {
                    "[]"
                }
            }),
        )
        .layer(from_fn_with_state(Arc::new(guard), middleware));
    TestServer::new(router)
}

#[tokio::test]
async fn a_test_token_reaches_the_handler_with_its_tenant() {
    let keys = TestKeys::new();
    let server = server(&keys);
    let acme = keys
        .token(ISSUER, AUDIENCE)
        .attr("tenant", ["acme"])
        .bearer();
    let response = server
        .get("/api/todos/1")
        .add_header("authorization", acme)
        .await;
    assert_eq!(response.status_code(), StatusCode::OK);
    assert_eq!(response.text(), "Acme's plan");
}

#[tokio::test]
async fn another_tenant_sees_nothing_of_acmes() {
    let keys = TestKeys::new();
    let server = server(&keys);
    let globex = keys
        .token(ISSUER, AUDIENCE)
        .attr("tenant", ["globex"])
        .bearer();
    assert_hidden(
        &server,
        &[("authorization", globex.as_str())],
        &[
            Probe::get("/api/todos/1"),
            Probe::list("/api/todos", "Acme"),
        ],
    )
    .await;
}

#[tokio::test]
async fn tokens_the_guard_must_refuse_are_refused() {
    let keys = TestKeys::new();
    let server = server(&keys);
    let refused = [
        // Signed by another key.
        TestKeys::from_seed([7; 32])
            .token(ISSUER, AUDIENCE)
            .attr("tenant", ["acme"])
            .bearer(),
        // For another service.
        keys.token(ISSUER, "other-service")
            .attr("tenant", ["acme"])
            .bearer(),
        // Expired.
        keys.token(ISSUER, AUDIENCE)
            .attr("tenant", ["acme"])
            .expires_at(1_700_000_001)
            .bearer(),
    ];
    for bearer in refused {
        let response = server
            .get("/api/todos/1")
            .add_header("authorization", bearer)
            .await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    }
}
