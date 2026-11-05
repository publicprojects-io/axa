//! # axa
//!
//! Small shared library for services built directly on Axum and sqlx.
//!
//! This is a library, not a framework: a service's `main.rs` stays ordinary
//! Axum and calls into this crate. There are no lifecycle hooks, application
//! context types, or macros.
