//! The integration tests, in one binary, as one link is faster than one per file.
//!
//! `audit_log` and `tenant_span` are binaries of their own, named in `Cargo.toml` with
//! `autotests = false`, as `tracing` caches per process whether a log site is on.

mod guard;
mod idp;
mod independence;
mod tenant;
