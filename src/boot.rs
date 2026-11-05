//! What a service exposes so tests can boot it.

use std::future::Future;

use axum::Router;

use crate::{AppContext, Result};

/// A booted service, ready to test.
#[derive(Debug, Clone)]
pub struct BootResult {
    /// The service's context.
    pub app_context: AppContext,
    /// The service's full router.
    pub router: Option<Router>,
}

/// A service that can boot for tests.
pub trait TestApp {
    /// Boot under the `test` environment.
    fn boot_test() -> impl Future<Output = Result<BootResult>>;
}
