//! # axa
//!
//! Accessories library for Axum websites, databases, microservices, and more.
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
//! # #[cfg(axa_db = "none")] {
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
//! | `postgres-seaorm` | A SeaORM-backed [`AppContext`] on Postgres, with `axa-postgres-seaorm` as `axa::postgres_seaorm` |
//! | `redis` | A Redis cache with mandatory expiry, as `axa::redis` |
//! | `postgres-diesel` | Diesel on Postgres in pure Rust, as `axa::postgres_diesel` |
//! | `postgres-toasty` | Toasty on Postgres, as `axa::postgres_toasty` |
//! | `tera` | Tera templates with safe escaping, as `axa::tera` |
//! | `tracing` | Redaction, request tracking, and asynchronous diagnostics, as `axa::tracing` |
//! | `session` | Web sessions with safe defaults, as `axa::session` |
//! | `postgres-sqlx` | sqlx on Postgres: pooling, migrations, and test resets, as `axa::postgres_sqlx` |
//! | `idempotency` | `Idempotency-Key` handling over Postgres, as `axa::idempotency` |
//! | `openapi` | `OpenAPI` documents, a pinned Swagger UI, and drift checks, as `axa::openapi` |
//! | `opentelemetry` | OTLP export of traces and request metrics, as `axa::opentelemetry` |
//! | `outbox` | A transactional outbox and relay on Postgres that keeps a record, as `axa::outbox` |
//! | `outbox-encryption` | `outbox`, with payloads sealed at rest under the service's keys |
//! | `cursor-pagination` | Signed cursor pages with no total, and `LIKE` escaping, as `axa::cursor_pagination` |
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
#[cfg(all(doctest, axa_db = "none"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// Compiles the Rust examples in the guides, so they cannot go stale.
#[cfg(all(doctest, feature = "queue"))]
#[doc = include_str!("../help/background-jobs/index.md")]
struct GuideBackgroundJobs;

/// Compiles the Rust examples in the idempotency keys guide.
#[cfg(all(doctest, feature = "idempotency", feature = "paseto"))]
#[doc = include_str!("../help/idempotency-keys/index.md")]
struct GuideIdempotencyKeys;

/// Compiles the Rust examples in the skill. Its first example passes `()` as the
/// database, so it builds only with no database feature, as the README's do.
#[cfg(all(doctest, feature = "idempotency", feature = "paseto", axa_db = "none"))]
#[doc = include_str!("../skill/axa-skill/SKILL.md")]
struct SkillDoctests;

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
#[cfg(feature = "model")]
pub mod model;
pub mod prelude;
#[cfg(feature = "queue")]
pub mod queue;
pub mod routes;
pub mod server;
pub mod store;
pub mod task;
#[cfg(feature = "model")]
pub mod validation;
pub mod worker;

#[cfg(feature = "versioning")]
pub use axa_accepts_version_negotiation as versioning;
#[cfg(feature = "cursor-pagination")]
pub use axa_cursor_pagination as cursor_pagination;
pub use axa_environment as environment;
#[cfg(feature = "idempotency")]
pub use axa_idempotency_key as idempotency;
#[cfg(feature = "keycloak")]
pub use axa_keycloak_auth as keycloak;
#[cfg(feature = "mysql-diesel")]
pub use axa_mysql_diesel as mysql_diesel;
#[cfg(feature = "mysql-seaorm")]
pub use axa_mysql_seaorm as mysql_seaorm;
#[cfg(all(feature = "mysql-seaorm", not(feature = "postgres-seaorm")))]
pub use axa_mysql_seaorm::{query, schema};
#[cfg(feature = "mysql-sqlx")]
pub use axa_mysql_sqlx as mysql_sqlx;
#[cfg(feature = "mysql-toasty")]
pub use axa_mysql_toasty as mysql_toasty;
#[cfg(feature = "openapi")]
pub use axa_openapi_docs as openapi;
#[cfg(feature = "opentelemetry")]
pub use axa_opentelemetry as opentelemetry;
#[cfg(feature = "paseto")]
pub use axa_paseto_auth as paseto;
#[cfg(feature = "postgres-diesel")]
pub use axa_postgres_diesel as postgres_diesel;
#[cfg(feature = "postgres-seaorm")]
pub use axa_postgres_seaorm as postgres_seaorm;
#[cfg(feature = "postgres-seaorm")]
pub use axa_postgres_seaorm::{query, schema};
#[cfg(feature = "postgres-sqlx")]
pub use axa_postgres_sqlx as postgres_sqlx;
#[cfg(feature = "postgres-toasty")]
pub use axa_postgres_toasty as postgres_toasty;
pub use axa_problem as problem;
#[cfg(feature = "prometheus")]
pub use axa_prometheus_metrics as prometheus;
#[cfg(feature = "redis")]
pub use axa_redis_cache as redis;
#[cfg(feature = "ratelimit")]
pub use axa_request_rate_limit as ratelimit;
#[cfg(feature = "sqlite-diesel")]
pub use axa_sqlite_diesel as sqlite_diesel;
#[cfg(feature = "sqlite-seaorm")]
pub use axa_sqlite_seaorm as sqlite_seaorm;
#[cfg(all(
    feature = "sqlite-seaorm",
    not(any(feature = "postgres-seaorm", feature = "mysql-seaorm"))
))]
pub use axa_sqlite_seaorm::{query, schema};
#[cfg(feature = "sqlite-sqlx")]
pub use axa_sqlite_sqlx as sqlite_sqlx;
#[cfg(feature = "tera")]
pub use axa_tera_templates as tera;
#[cfg(feature = "tracing")]
pub use axa_tracing as tracing;
#[cfg(feature = "outbox")]
pub use axa_transactional_outbox as outbox;
#[cfg(feature = "session")]
pub use axa_web_session as session;
pub use context::AppContext;
pub use environment::Environment;
pub use error::{Error, Result};
