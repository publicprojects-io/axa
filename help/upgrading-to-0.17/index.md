# Upgrading to Axa 0.17

This covers a service on 0.16. Most services upgrade by changing the version. A service
that uses Diesel or sessions follows those libraries' own upgrades, and one whose
database role cannot alter tables reads the shared rate limit's note below.

## What will not compile

| Change | What to do |
|---|---|
| Rust 1.98 or newer | `rustup update`. A service's own `rust-version` may stay lower only if it builds no Axa crate |
| `axa-test` 0.8 is the one for `axa` 0.17 | Set `axa-test = "0.8"` in `[dev-dependencies]` |
| `axa::postgres_diesel` and `axa::mysql_diesel` re-export Diesel 2.4, `diesel-async` 0.10, and `diesel_migrations` 2.4, and the MySQL crate `mysql_async` 0.37 | A service that names these crates' types through Axa, or depends on them itself, moves to the same versions and follows their changelogs. One that uses only `Pool`, `connect`, and `migrate` changes nothing |
| `axa::session` re-exports `axum_session` 0.23 and `axum_session_sqlx` 0.13 | A service that depends on `axum_session` itself moves to 0.23. One that uses only `axa::session` changes nothing |

## What changes with no compile error

**An empty public prefix opens nothing**, in the PASETO and Keycloak guards. Before, a
prefix of `""`, as a trailing comma in a configured list leaves, made every path
public. A service that relied on that, knowingly or not, now asks for a token on every
path but the ones it lists. Check the public paths a service builds from configuration.

**A Keycloak refusal says only `invalid token`**, for a missing header, one that is not
a bearer token, and a route without the middleware, as for a bad token. The reason is
logged. A client that read the old `detail` text reads the status instead.

**A shared rate limit counts a client under a new key.** Keys are now HMAC-SHA256 under
the secret, the same from every build, so every client's count starts again once, for
one window, after the upgrade. On Postgres, `Store::postgres` adds a `window_seconds`
column to `axa_rate_limits` with `ADD COLUMN IF NOT EXISTS`, so the role it connects as
must own the table, as it already must to make it. `Store::purge` now ends each limit's
windows by its own length, so a short purge no longer resets a longer limit.

**A job whose heartbeat fails stops sooner.** In the three job queues, a heartbeat waits
at most `heartbeat_interval`, and a run that cannot report in stops two heartbeats before
`stale_minutes`, or at half of it, rather than at `stale_minutes`, so it has stopped
before another worker may take it. Its job goes to `needs_review`, as before.

**A job still running at the shutdown time limit is stopped**, rather than left running
detached until the process exits.

**The internal port takes no PROXY header.** With `PROXY_PROTOCOL_FROM` covering the
probes' addresses, plain probes and scrapes to `INTERNAL_PORT` now work. The main port
is unchanged. `MAX_CONNECTIONS` is the limit on each listener, as it was.

**An idempotent replay sends every value of a repeated header**, such as two `link`s,
rather than the first.

**A `SHUTDOWN_DRAIN_SECONDS` too large for the clock drains for ever**, rather than
panicking at shutdown.
