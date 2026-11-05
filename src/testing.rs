//! Request-level test helpers.

use std::{future::Future, net::SocketAddr};

pub use axum_test::TestServer;

pub use crate::boot::{BootResult, TestApp};
use crate::{AppContext, Result};

/// Boot `H` for tests.
///
/// # Errors
///
/// Propagates the boot failure.
pub async fn boot_test<H: TestApp>() -> Result<BootResult> {
    H::boot_test().await
}

/// Boot `H`, then run `callback` with a test client and the context.
///
/// # Panics
///
/// If the service cannot boot.
pub async fn request<H, F, Fut>(callback: F)
where
    H: TestApp,
    F: FnOnce(TestServer, AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    let boot = H::boot_test().await.unwrap();
    let router = boot.router.clone().unwrap();
    let server = TestServer::new(router.into_make_service_with_connect_info::<SocketAddr>());
    callback(server, boot.app_context).await;
}

/// Everything a request test needs.
pub mod prelude {
    pub use super::{BootResult, TestApp, TestServer, boot_test, request};
}
