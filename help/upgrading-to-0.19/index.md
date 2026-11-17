# Upgrading to Axa 0.19

This covers a service on 0.18. A service that uses no job queue upgrades by changing the
version, unless it read sqlx columns as `chrono` types through Axa's features, below.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.10 is the one for `axa` 0.19 | Set `axa-test = "0.10"` in `[dev-dependencies]` |
| `axa::queue::{DateTime, Utc}` are gone | Use `axa::queue::Timestamp`, which is `jiff::Timestamp`. `DateTime<Utc>` becomes `Timestamp` |
| `NewJob::run_at` and `Schedule::first_at` take a `Timestamp` | Make one with `Timestamp::now()`, `Timestamp::from_second(secs)?`, `"2026-11-17T02:00:00Z".parse()?`, or `now + jiff::SignedDuration::from_hours(1)`. From a `chrono::DateTime<Utc>` the service still holds: `Timestamp::from_microsecond(at.timestamp_micros())?` |
| The times of `JobInfo`, `ScheduleInfo`, and `Act` are `Timestamp` | `a - b` on two of them is a `jiff::Span`: use `a.duration_since(b)`, a `SignedDuration`, for `num_milliseconds()` read `as_millis()`, for `num_minutes()` read `as_mins()`. Format with `to_string()`, which is RFC 3339 in UTC |
| `axa-postgres-job-queue` 0.6, `axa-mysql-job-queue` 0.4 | The same change, for a service that uses the crates on their own |
| sqlx's `chrono` feature is no longer on through `axa` | A service that decodes or binds `chrono` types with sqlx adds `features = ["chrono"]` to its own `sqlx` dependency |

## What changes with no compile error

Nothing. Times are stored as before, on the database's clock in UTC, and cron schedules
are still evaluated in UTC, so no table changes and no migration is needed.

## Why

`chrono` is unmaintained (RUSTSEC-2026-0543). A service on Postgres with Axa's `queue`
feature now builds none, so it can clear the advisory once its own code needs none.
