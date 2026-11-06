//! Redaction, request tracking, and asynchronous diagnostics over `tracing`.
//!
//! `axa` already logs: it installs the subscriber, opens a span for each request, and
//! puts the request id on every line written while serving it. This crate adds what
//! a service needs next, and what that span cannot reach by itself.
//!
//! | Need | Use |
//! |---|---|
//! | Keep a secret out of a log field | [`Redacted`] |
//! | Find the spans that took too long | [`SlowSpans`] |
//! | Read the request id deep in the code | [`track`], [`current_request_id`] |
//! | Send the request id to a peer service | [`propagate`] |
//! | Keep a request's id and span in a spawned task | [`spawn`] |
//! | Catch a future that blocks the runtime | [`Watch::watch`] |
//! | Notice a stalled runtime | [`monitor_runtime`] |
//! | Log panics, in spawned tasks too | [`log_panics`] |
//!
//! None of this needs `tokio_unstable`, which is why it reports less than
//! `tokio-console` does: a service can run it in production.
//!
//! # Use
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use axa_tracing::{Redacted, SlowSpans, Watch, current_request_id, track};
//! use axum::{Router, middleware::from_fn, routing::get};
//!
//! async fn handler() -> String {
//!     tracing::info!(token = %Redacted("secret"), "signing in");
//!     load().watch("load", Duration::from_millis(100)).await;
//!     current_request_id().unwrap_or_default()
//! }
//!
//! async fn load() {}
//!
//! # fn example() {
//! axa_tracing::log_panics();
//! let app: Router = Router::new()
//!     .route("/", get(handler))
//!     .layer(from_fn(track));
//! let slow_spans = SlowSpans::new(Duration::from_secs(1));
//! # let _ = (app, slow_spans);
//! # }
//! ```
//!
//! Hand [`SlowSpans`] to `axa::logger::init_with`, so that it sits beside the
//! service's log format and filter.

mod diagnostics;
mod logging;
mod request;
#[cfg(test)]
mod test_support;

pub use diagnostics::{
    RuntimeMonitor, RuntimeSnapshot, Watch, Watched, log_panics, monitor_runtime, runtime_snapshot,
};
pub use logging::{Redacted, SlowSpans};
pub use request::{X_REQUEST_ID, current_request_id, propagate, spawn, track};
