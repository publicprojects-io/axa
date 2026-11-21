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

/// Settings for a test to boot with, read as [`crate::config::Config::from_lookup`]
/// reads them, in place of the process environment.
pub type Lookup = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A service that can boot for tests.
pub trait TestApp {
    /// Boot under the `test` environment.
    fn boot_test() -> impl Future<Output = Result<BootResult>>;

    /// Boot under the `test` environment with settings read through `lookup` rather
    /// than the process environment, so that tests need not set variables, which every
    /// thread shares, and need not run one at a time. A service whose boot reads its
    /// configuration with [`crate::config::Config::from_lookup`] passes `lookup` there.
    ///
    /// # Errors
    ///
    /// By default, always: a service that does not implement this would otherwise boot
    /// with settings other than the ones the test gave.
    fn boot_test_with(lookup: Lookup) -> impl Future<Output = Result<BootResult>> {
        let _ = lookup;
        std::future::ready(Err(crate::Error::Message(
            "this service does not boot from given settings: implement TestApp::boot_test_with"
                .to_string(),
        )))
    }
}
