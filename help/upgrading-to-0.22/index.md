# Upgrading to Axa 0.22

This covers a service on 0.21. A service that calls none of the functions below, and sets
none of the job queue or cache settings out of range, upgrades by changing the versions.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.13 is the one for `axa` 0.22 | Set `axa-test = "0.13"` in `[dev-dependencies]` |
| `Queue::from_pool` in each job queue returns `Result` | Add a `?`. It refuses settings out of range, as `connect` does |
| `Schedule::every`, `Schedule::cron`, `Queue::schedule` in each job queue, and `axa::queue::Queue::schedule`, return `Result` | Add a `?`. A bad name, an interval under a second, a bad or empty cron pattern, or a name added twice is the new `Error::Schedule` |
| `RetryAfter::new` returns `Result` | Add a `?`. A delay under a second or over an hour is `Error::OutOfRange` |
| `backoff`, `retry_wait` and `jittered` in each job queue return `Option<Duration>` | Handle `None`: `backoff` and `retry_wait` for fewer than one attempt or a delay out of range, `jittered` for a spread that does not fit |
| Each job queue's `Error` has `OutOfRange`, `Overflow` and `Schedule`, and `axa_redis_cache::Error` has `Overflow` | Add an arm to a `match` on one of them, or a wildcard |
| `Idempotency::ttl` and `rerun_abandoned_after` return `Result<_, ConfigError>`, and are not `const` | Add a `?`, and build the value at run time |
| `ApiVersions::new` returns `Result<_, CurrentNotSupported>` | Add a `?`: the current version must be one of those supported |
| `PoolConfig::builder` in `axa-postgres-toasty` and `axa-mysql-toasty` returns `Result` | Add a `?`: `max_connections` must fit the pool's size |

## What changes with no compile error

| Change | What to do |
|---|---|
| A job queue refuses settings out of range at `connect` | Held to a limit before. A `heartbeat_interval` under 20 ms or over a quarter of `stale_minutes`, a `stale_minutes` under 1, or `workers` past a `u32` is `Error::OutOfRange`. A queue that booted with such a setting does not boot now: run `service check` with the deploy config |
| A run gives up exactly two heartbeats before its stale time | There is no half-way fallback. Nothing to do unless a test counted on the old time |
| Input out of range or malformed has no fallback | An upload past the limits is refused as too large. A cursor or page size past what SQL holds is `PageError::BadCursor`. A session signed in at a later time than now is destroyed. A metric whose counter went backwards adds nothing. An outbox age below zero is `None` in `Stats` |
| `logger::init_env_filter` and `init_with` no longer panic on an invalid filter | The bad override is passed over, as a bad `RUST_LOG` is, and the directive dropped |
| `server::shutdown_signal` no longer panics when a signal handler cannot be installed | It logs the error, and that signal never fires |
| `axa_openapi_docs::spec_of` answers `null` for a document that cannot be encoded | It panicked before |
| The Toasty crates read a row of the wrong shape as an error | A table name that is not text, a migration id that is not a number, or a missing count was skipped or read as zero. Migrations no longer run again because of it |
| `axa-test`'s Postgres template is named from a SHA-256 of the migrations | Each developer's templates are built again once. The `postgres-sqlx` feature of `axa-test` turns on `sha2` |

## What a service may delete

| Its own | Axa's in its place |
|---|---|
| A `.unwrap()` on a schedule built at boot | The `?` the function now allows. A bad schedule stops the boot with a named error |
