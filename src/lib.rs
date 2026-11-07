//! # axa
//!
//! Small shared library for services built directly on Axum.
//!
//! This is a library, not a framework. A service's `main.rs` stays ordinary
//! Axum and calls into this crate: there are no lifecycle hooks or macros, and
//! [`AppContext`] is a plain struct passed as ordinary Axum state. Every piece
//! can be read, replaced, or left out on its own.
//!
//! # Example
//!
//! Declare routes, turn them into a plain [`axum::Router`] under the standard
//! middleware, and serve it:
//!
//! ```
//! use axa::prelude::*;
//! use axa::config::{Config, Defaults};
//! use axa::Environment;
//!
//! async fn hello() -> Result<Response> {
//!     format::json(serde_json::json!({ "hello": "world" }))
//! }
//!
//! # fn main() -> Result<()> {
//! let environment = Environment::Test;
//! let config = Config::from_lookup(
//!     &environment,
//!     &Defaults { database: "", ..Defaults::default() },
//!     |_| None,
//! )?;
//! # #[cfg(not(any(feature = "sea-orm", feature = "sqlx")))] {
//! let ctx = AppContext::new(environment, (), config);
//! let router = AppRoutes::with_default_routes()
//!     .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
//!     .into_app(ctx)?;
//! # let _: axum::Router = router;
//! # }
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! | Feature | Adds |
//! |---|---|
//! | `queue` | The Postgres job queue in [`queue`] |
//! | `testing` | Request-level test helpers in [`testing`] |
//! | `sea-orm` | A SeaORM-backed [`AppContext`], with `axa-seaorm` as `axa::seaorm` |
//! | `redis` | A Redis cache with mandatory expiry, as `axa::redis` |
//! | `diesel` | Diesel pooling, migrations, and test resets in pure Rust, as `axa::diesel` |
//! | `tera` | Tera templates with safe escaping, as `axa::tera` |
//! | `tracing` | Redaction, request tracking, and asynchronous diagnostics, as `axa::tracing` |
//! | `session` | Web sessions with safe defaults, as `axa::session` |
//! | `sqlx` | Pooling, migrations, and test resets, as `axa::sqlx` |
//! | `openapi` | `OpenAPI` documents, a pinned Swagger UI, and drift checks, as `axa::openapi` |
//! | `opentelemetry` | OTLP export of traces and request metrics, as `axa::opentelemetry` |
//! | `pagination` | Offset pagination and `LIKE` escaping, as `axa::pagination` |
//! | `keycloak` | Keycloak bearer authentication and ABAC enforcement, as `axa::keycloak` |
//! | `paseto` | PASETO bearer authentication and ABAC enforcement, as `axa::paseto` |
//! | `problem` | Nothing: RFC 9457 problem details are always `axa::problem`, as [`Error`] answers with them |
//! | `prometheus` | Prometheus text exposition, as `axa::prometheus` |
//! | `ratelimit` | In-process request limits and failed-login lockout, as `axa::ratelimit` |
//! | `versioning` | `Accepts-version` negotiation, as `axa::versioning` |
//!
//! # Origin
//!
//! The behaviours here are the ones the Main X Index services relied on in
//! [Loco](https://loco.rs). They were written afresh rather than copied, so
//! this crate can keep its own licence terms.

/// Compiles the Rust examples in `README.md`, so they cannot go stale.
#[cfg(all(doctest, not(any(feature = "sea-orm", feature = "sqlx"))))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub mod boot;
pub mod cli;
pub mod client;
pub mod conditional;
pub mod config;
pub mod context;
pub mod db;
pub mod error;
pub mod extract;
pub mod format;
pub mod health;
pub mod logger;
pub mod middleware;
pub mod prelude;
#[cfg(feature = "queue")]
pub mod queue;
pub mod routes;
pub mod server;
pub mod store;
pub mod task;
#[cfg(feature = "testing")]
pub mod testing;
pub mod worker;

#[cfg(feature = "diesel")]
pub use axa_diesel as diesel;
pub use axa_environment as environment;
#[cfg(feature = "keycloak")]
pub use axa_keycloak as keycloak;
#[cfg(feature = "openapi")]
pub use axa_openapi as openapi;
#[cfg(feature = "opentelemetry")]
pub use axa_opentelemetry as opentelemetry;
#[cfg(feature = "pagination")]
pub use axa_pagination as pagination;
#[cfg(feature = "paseto")]
pub use axa_paseto as paseto;
pub use axa_problem as problem;
#[cfg(feature = "prometheus")]
pub use axa_prometheus as prometheus;
#[cfg(feature = "ratelimit")]
pub use axa_ratelimit as ratelimit;
#[cfg(feature = "redis")]
pub use axa_redis as redis;
#[cfg(feature = "sea-orm")]
pub use axa_seaorm as seaorm;
#[cfg(feature = "sea-orm")]
pub use axa_seaorm::{model, query, schema, validation};
#[cfg(feature = "session")]
pub use axa_session as session;
#[cfg(feature = "sqlx")]
pub use axa_sqlx as sqlx;
#[cfg(feature = "tera")]
pub use axa_tera as tera;
#[cfg(feature = "tracing")]
pub use axa_tracing as tracing;
#[cfg(feature = "versioning")]
pub use axa_versioning as versioning;
pub use context::AppContext;
pub use environment::Environment;
pub use error::{Error, Result};
#[cfg(feature = "testing")]
pub use testing::TestServer;
