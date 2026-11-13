//! What `assert_hidden` accepts and refuses, against a tiny service with one route
//! that keeps tenants apart and one that does not.

use std::collections::HashMap;

use axa_test::{
    TestServer,
    isolation::{Probe, assert_hidden},
};
use axum::{
    Router,
    extract::Path,
    http::{HeaderMap, StatusCode},
    routing::get,
};

/// The owner of each id: id 1 is acme's.
fn owner(id: u32) -> Option<&'static str> {
    HashMap::from([(1, "acme")]).get(&id).copied()
}

fn tenant(headers: &HeaderMap) -> String {
    headers["x-tenant"].to_str().unwrap().to_string()
}

fn server() -> TestServer {
    let router = Router::new()
        // Kept apart: another tenant's id is a 404, like a missing one.
        .route(
            "/kept/{id}",
            get(|headers: HeaderMap, Path(id): Path<u32>| async move {
                match owner(id) {
                    Some(owner) if owner == tenant(&headers) => (StatusCode::OK, "plan"),
                    _ => (StatusCode::NOT_FOUND, ""),
                }
            })
            .delete(|headers: HeaderMap, Path(id): Path<u32>| async move {
                match owner(id) {
                    Some(owner) if owner == tenant(&headers) => StatusCode::OK,
                    _ => StatusCode::NOT_FOUND,
                }
            }),
        )
        .route(
            "/kept",
            get(|headers: HeaderMap| async move {
                if tenant(&headers) == "acme" {
                    "[1]"
                } else {
                    "[]"
                }
            }),
        )
        // Leaky: it says the id exists, and lists every tenant's.
        .route(
            "/leaky/{id}",
            get(|Path(id): Path<u32>| async move {
                match owner(id) {
                    Some(_) => (StatusCode::FORBIDDEN, ""),
                    None => (StatusCode::NOT_FOUND, ""),
                }
            }),
        )
        .route("/leaky", get(|| async { "[1]" }));
    TestServer::new(router)
}

#[tokio::test]
async fn routes_that_keep_tenants_apart_pass() {
    let server = server();
    assert_hidden(
        &server,
        &[("x-tenant", "globex")],
        &[
            Probe::get("/kept/1"),
            Probe::delete("/kept/1"),
            Probe::list("/kept", "1"),
        ],
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "GET /leaky/1 answered 403 Forbidden to another tenant")]
async fn a_403_tells_another_tenant_the_id_exists() {
    let server = server();
    assert_hidden(
        &server,
        &[("x-tenant", "globex")],
        &[Probe::get("/leaky/1")],
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "GET /leaky shows \"1\" to another tenant")]
async fn a_list_that_shows_another_tenants_id_fails() {
    let server = server();
    assert_hidden(
        &server,
        &[("x-tenant", "globex")],
        &[Probe::list("/leaky", "1")],
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "GET /kept/1 answered 200 OK to another tenant")]
async fn the_owner_is_not_another_tenant() {
    // Probing as the owner is a mistake in the test, and it says so.
    let server = server();
    assert_hidden(&server, &[("x-tenant", "acme")], &[Probe::get("/kept/1")]).await;
}
