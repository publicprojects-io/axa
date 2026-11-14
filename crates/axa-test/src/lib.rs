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
#[cfg(feature = "paseto")]
pub mod paseto;

pub use axa::boot::{BootResult, TestApp};
use axa::{AppContext, Result};
pub use axum_test::TestServer;

/// Boot `H` for tests.
///
/// # Errors
///
/// Propagates the boot failure.
pub async fn boot_test<H: TestApp>() -> Result<BootResult> {
    H::boot_test().await
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
    let boot = H::boot_test().await.expect("the service boots for tests");
    let router = boot.router.clone().expect("the service has a router");
    let server = TestServer::new(router.into_make_service_with_connect_info::<SocketAddr>());
    callback(server, boot.app_context).await;
}

/// Everything a request test needs.
pub mod prelude {
    pub use super::{BootResult, TestApp, TestServer, boot_test, request};
}
