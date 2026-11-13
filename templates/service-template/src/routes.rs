//! The service's routes.
//!
//! Add a group of routes with `Routes::new().prefix(...)`, and return
//! `axa::Result<Response>` from each handler, so that every error is an RFC 9457
//! problem that quotes nothing the caller sent.

use axa::{
    Result,
    prelude::{AppRoutes, Response, Routes, format, get},
};
use serde::Serialize;

/// What `/api/v1/status` says.
#[derive(Serialize)]
struct Status {
    version: &'static str,
}

/// The running version. Replace this with the service's first real route.
async fn status() -> Result<Response> {
    format::json(Status {
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Every route, with `/_health/live`, `/_health/ready`, and `/_health/startup` from Axa.
pub fn routes() -> AppRoutes {
    AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api/v1").add("/status", get(status)))
}
