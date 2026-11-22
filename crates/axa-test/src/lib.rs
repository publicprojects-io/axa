#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "RCA-SCOPE-4: test-support code, which may use plain operators and index"
)]
//! Request-level test helpers for `axa` services, built on
//! [`axum-test`](https://docs.rs/axum-test).
//!
//! A service implements [`TestApp`] to say how it boots for tests, then sends real
//! requests to its full router, middleware and all, with no port opened:
//!
//! ```ignore
//! use axa_test::prelude::*;
//!
//! #[tokio::test]
//! async fn health_answers() {
//!     request::<App, _, _>(|server, _ctx| async move {
//!         assert_eq!(server.get("/_health/live").await.status_code(), 200);
//!     })
//!     .await;
//! }
//! ```
//!
//! Add it as a dev-dependency, so it never ships in a service's binary.

use std::{future::Future, net::SocketAddr};

pub mod isolation;
pub mod logs;
#[cfg(feature = "mail")]
pub mod mail;
#[cfg(feature = "openapi")]
pub mod openapi;
pub mod own_process;
#[cfg(feature = "paseto")]
pub mod paseto;
#[cfg(feature = "postgres-sqlx")]
pub mod postgres;

pub use axa::boot::{BootResult, Lookup, TestApp};
use axa::{AppContext, Result};
pub use axum_test::TestServer;

/// Boot `H` for tests.
///
/// # Errors
///
/// Propagates the boot failure.
pub async fn boot_test<H: TestApp>() -> Result<BootResult> {
    axa::tls::install_provider();
    H::boot_test().await
}

/// Boot `H` for tests with the settings `pairs` names and no others, through
/// [`TestApp::boot_test_with`].
///
/// # Errors
///
/// Propagates the boot failure, and fails for a service that does not implement
/// [`TestApp::boot_test_with`].
pub async fn boot_test_with<H: TestApp>(pairs: &[(&str, &str)]) -> Result<BootResult> {
    axa::tls::install_provider();
    H::boot_test_with(lookup(pairs)).await
}

/// Boot `H`, then run `callback` with a test client for its router and its context.
///
/// # Panics
///
/// If the service cannot boot, or boots with no router.
pub async fn request<H, F, Fut>(callback: F)
where
    H: TestApp,
    F: FnOnce(TestServer, AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    axa::tls::install_provider();
    let boot = H::boot_test().await.expect("the service boots for tests");
    let router = boot.router.clone().expect("the service has a router");
    let server = TestServer::new(router.into_make_service_with_connect_info::<SocketAddr>());
    callback(server, boot.app_context).await;
}

/// [`request`], booting `H` with the settings `pairs` names and no others, through
/// [`TestApp::boot_test_with`], so that a test needs no process environment.
///
/// # Panics
///
/// If the service cannot boot, does not implement [`TestApp::boot_test_with`], or boots
/// with no router.
pub async fn request_with<H, F, Fut>(pairs: &[(&str, &str)], callback: F)
where
    H: TestApp,
    F: FnOnce(TestServer, AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    axa::tls::install_provider();
    let boot = H::boot_test_with(lookup(pairs))
        .await
        .expect("the service boots for tests with the settings given");
    let router = boot.router.clone().expect("the service has a router");
    let server = TestServer::new(router.into_make_service_with_connect_info::<SocketAddr>());
    callback(server, boot.app_context).await;
}

/// A [`Lookup`] that finds exactly `pairs`, and nothing else.
#[must_use]
pub fn lookup(pairs: &[(&str, &str)]) -> Lookup {
    let pairs: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    std::sync::Arc::new(move |name| pairs.get(name).cloned())
}

/// Everything a request test needs.
pub mod prelude {
    pub use super::{
        BootResult, Lookup, TestApp, TestServer, boot_test, boot_test_with, lookup, request,
        request_with,
    };
}
