//! # axa
//!
//! Small shared library for services built directly on Axum and sqlx.
//!
//! This is a library, not a framework: a service's `main.rs` stays ordinary
//! Axum and calls into this crate. There are no lifecycle hooks or macros.
//! [`AppContext`] is a plain struct passed as ordinary Axum state.
//!
//! The behaviours here were modelled on those the Main X Index family relied
//! on in [Loco](https://loco.rs), written afresh rather than copied. See the
//! crate `README.md`.

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
