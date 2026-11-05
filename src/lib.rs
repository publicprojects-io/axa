//! # axa
//!
//! Small shared library for services built directly on Axum and sqlx.
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
//! # #[cfg(not(feature = "sea-orm"))] {
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
//! | `sea-orm` | SeaORM-backed [`AppContext`], `model`, and `schema`. Transitional: it goes once the last service has moved to sqlx |
//!
//! # Origin
//!
//! The behaviours here are the ones the Main X Index services relied on in
//! [Loco](https://loco.rs). They were written afresh rather than copied, so
//! this crate can keep its own licence terms.

pub mod boot;
pub mod cli;
pub mod config;
pub mod context;
pub mod db;
pub mod environment;
pub mod error;
pub mod format;
pub mod health;
pub mod logger;
pub mod middleware;
#[cfg(feature = "sea-orm")]
pub mod model;
pub mod prelude;
#[cfg(feature = "queue")]
pub mod queue;
pub mod routes;
#[cfg(feature = "sea-orm")]
pub mod schema;
pub mod server;
pub mod store;
pub mod task;
#[cfg(feature = "testing")]
pub mod testing;
pub mod worker;

pub use context::AppContext;
pub use environment::Environment;
pub use error::{Error, Result};
#[cfg(feature = "testing")]
pub use testing::TestServer;
