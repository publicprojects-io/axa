#![allow(clippy::indexing_slicing, reason = "RCA-SCOPE-4: tests may index")]
//! The integration tests, in one binary, as one link is faster than one per file.
//!
//! `logs_and_mail` and `postgres_templates` are binaries of their own, named in
//! `Cargo.toml` with `autotests = false`: the first because `tracing` caches per process
//! whether a log site is on, and the second because it drops the templates the tests in
//! `postgres.rs` clone.

mod boot;
mod isolation;
mod paseto;
mod postgres;
