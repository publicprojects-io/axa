//! The integration tests, in one binary, as one link is faster than one per file.
//!
//! A test that needs a process of its own, because it starts this binary again as a
//! child or because `tracing` caches per process whether a log site is on, is a binary
//! of its own, named in `Cargo.toml` with `autotests = false`.

mod client_address;
mod conditional;
mod constraint_errors;
mod idempotency;
mod maud_policy;
mod minijinja_policy;
mod other_databases;
mod ratelimit;
mod request_tracking;
mod row_security;
mod stale_plan;
mod tera_policy;
