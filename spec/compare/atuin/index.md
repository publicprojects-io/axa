# Atuin sync server

Atuin keeps shell history in SQLite on each machine and syncs it, end-to-end
encrypted, through a server. Its repository is
[github.com/atuinsh/atuin](https://github.com/atuinsh/atuin). I read the `main` branch at
commit
[`da74633`](https://github.com/atuinsh/atuin/tree/da746332055c20356e6bd2d2740fe71479055321)
of 10 November 2026. It has about 32,000 stars and is licensed MIT. The server is the
`crates/atuin-server` crate, about 4,000 lines including SQL and tests, with shared
pieces in `crates/atuin-common` and `crates/atuin-domain`. It is a small Axum 0.8 service
started with `axum::serve`, holding an `Arc<dyn DynDatabase>` that is backed by Postgres,
SQLite, or MySQL through sqlx. The server stores only encrypted blobs, so most of its
API is accounts, sessions, and record upload and download.

Links below are to files at that commit, under
`https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/`.

[lib]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/lib.rs
[router]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/router.rs
[metrics]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/metrics.rs
[trace]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/trace.rs
[clientip]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/client_ip.rs
[settings]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/settings.rs
[servertoml]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/server.toml
[handlers]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/handlers/mod.rs
[health]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/handlers/health.rs
[user]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/handlers/user.rs
[record]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/handlers/v0/record.rs
[db]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/db/mod.rs
[models]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/db/models.rs
[pg]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/db/postgres/mod.rs
[sqlite]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/db/sqlite/mod.rs
[admin]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/admin/user.rs
[main]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/src/bin/main.rs
[tests]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-server/tests/common/mod.rs
[commondb]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-common/src/db/mod.rs
[pool]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-common/src/sync/blocking_pool.rs
[caps]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-domain/src/caps/mod.rs
[capsaxum]: https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/crates/atuin-domain/src/caps/axum.rs

## How it is built

### App state and routing

`AppState` is a database handle and the settings ([`router.rs`][router]). Routes are
plain `Router::route` calls. Most of them sit inside a capability negotiation layer
(see below). An optional `path` setting nests the whole router under a prefix. The
fallback answers `404` with plain text. Outside everything sit three small layers: an
`X-Clacks-Overhead` header, request metrics, and a header carrying the server version.

### Middleware and tracing

Inside the router, a middleware resolves the client address, and then `tower-http`'s
`TraceLayer` opens an `http.request` span with method, route template, status, and
client address, and logs the latency at the end ([`trace.rs`][trace]). There is no
request id, no request timeout, no CORS, no security headers, no compression, and no
panic catching in the stack.

### Client address

[`client_ip.rs`][clientip] honours a forwarding header only when the TCP peer is in
`trusted_proxies`. It walks the header from right to left, skips trusted hops, and takes
the first untrusted address. The header name is configurable (`X-Forwarded-For` by
default, or `X-Real-IP`). IPv4-mapped IPv6 peers are made canonical first. This is close
to what axa does.

### Errors

Errors are `{"reason": "..."}` with a status picked in each handler
([`handlers/mod.rs`][handlers]). Database errors are logged and answered with a generic
reason. Login answers `404 user not found` for an unknown user and `401` for a wrong
password, so it tells a caller which usernames exist ([`handlers/user.rs`][user]).

### Auth and passwords

Clients send `Authorization: Token <token>`. The extractor looks the token up by
equality in the `sessions` table, which stores the token as issued ([`router.rs`][router],
[`db/mod.rs`][db]). Passwords are Argon2id. Hashing and checking run on a
`BlockingPool` that is Tokio's blocking pool behind a semaphore, capped at the number of
CPUs, so a burst of logins cannot take every blocking thread
([`db/models.rs`][models], [`atuin-common/src/sync/blocking_pool.rs`][pool]). The pool's
permit stays with the closure, not with the awaiting caller, so a client that hangs up
does not free a slot while the hash still runs. There is no rate limit or lockout on
login. Registration is off unless `open_registration` is set, and a registration can
call a webhook, awaited inside the request with a 5-second timeout.

### Request bodies and paging

The router sets no `DefaultBodyLimit`, so Axum's 2 MB default applies to the `Json`
extractor. `max_record_size` (1 GiB by default) is then checked per record after
parsing ([`handlers/v0/record.rs`][record]). The record download takes a `count`
query parameter that goes to SQL `LIMIT` without a cap that I could find, though the
server advertises a page size of 100 as a capability.

### Database and migrations

[`db/mod.rs`][db] defines an object-safe `DynDatabase` trait and a `Database` trait with
default methods, so one implementation serves three backends. The SQL lives as
constants on a `Dialect` trait, one implementation for `$1` placeholders (Postgres and
SQLite) and one for `?` (MySQL). Each backend runs `sqlx::migrate!` from its own
directory on connect, every time ([`postgres/mod.rs`][pg], [`sqlite/mod.rs`][sqlite]).
Postgres refuses to start below version 14. SQLite is opened in WAL mode.

`atuin-common`'s query wrappers panic, in debug builds only, on any query holding `*`
or `ALTER`, and the workspace's
[`clippy.toml`](https://github.com/atuinsh/atuin/blob/da746332055c20356e6bd2d2740fe71479055321/clippy.toml)
forbids calling `sqlx::query`, `query_as`, `query_scalar`, and `migrate!` directly
([`atuin-common/src/db/mod.rs`][commondb]). The reason it gives is that in sqlx-sqlite, a
cached `SELECT *` panics once an `ALTER` changes its column count. Its migrate macro clears the connection's
statement cache afterwards, citing a sqlx issue where cached statements outlive a
schema change.

### Configuration

Settings come from `server.toml`, then `ATUIN_*` variables, through the `config` crate
([`settings.rs`][settings]). The first run writes an example `server.toml` into the
config directory. `db_password_file` reads the database password from a file, such as a
container secret, and refuses to start if the URL also holds a password, the file is
empty, or the database is SQLite ([`server.toml`][servertoml], [`db/mod.rs`][db]). The
settings drop an `ATUIN_PORT` of the form `tcp://ip:port`, because a Kubernetes Service
named `atuin` injects that variable into every pod in its namespace.

### Health, metrics, shutdown

`/healthz` answers `200 {"status":"healthy"}` and asks nothing, not even the database
([`handlers/health.rs`][health]). Metrics use the `metrics` crate with
`metrics-exporter-prometheus`: `http_requests_total` and
`http_requests_duration_seconds` labelled by method, path, and status, plus counters
for users and records ([`metrics.rs`][metrics]). The path label is the route template,
but for a request that matched no route it is the raw path. Metrics are served at
`/metrics` on a separate listener, `127.0.0.1:9001` by default, which the code says is so
they are not exposed by accident ([`lib.rs`][lib]). Shutdown uses `axum::serve`'s
`with_graceful_shutdown` on `SIGTERM` or `SIGINT`, with no time limit.

### Admin and CLI

The binary has `start`, `default-config`, and `admin user create | set-password |
delete`. Passwords are read from a prompt, or from standard input when it is not a
terminal. `create` works when open registration is off ([`main.rs`][main],
[`admin/user.rs`][admin]).

### Capability negotiation

The server publishes a set of named, versioned capabilities and a hash of the set at
`/api/v0/capabilities` ([`caps/mod.rs`][caps], [`caps/axum.rs`][capsaxum]). A client
sends the hash it knows. If it is stale, the server either answers `412` (when the client
asked it to enforce) or serves the request and adds the current hash, so the client
refreshes in the background. Clients that send no hash are unaffected.

### Testing

Integration tests start the real server on a port-0 listener, wait 200 ms, and drive it
with Atuin's own client library against a live Postgres from `ATUIN_DB_URI`
([`tests/common/mod.rs`][tests]). Admin commands are tested by running the built binary.

## Compared with axa

| Area | Atuin server | axa |
|---|---|---|
| Routing | Plain `Router`, optional path prefix | `Routes` groups, prefixes, per-route timeouts, route census |
| Middleware | Trace, client address, metrics, version header | Request id, trace, panic catching, security headers, CORS, timeout, 304, compression, body limit |
| Errors | `{"reason"}`, status per handler, login tells which usernames exist | RFC 9457 problems, constraint violations mapped to 409 and 422 |
| Auth | Session token looked up by equality, stored as issued | PASETO, Keycloak, web sessions |
| Password hashing | Argon2id on a capped blocking pool | No password hashing helper |
| Login throttling | None | `lock_out_failures` in `axa-request-rate-limit` |
| Body limit | Axum's 2 MB default, plus a per-record check | `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, per-route override |
| Paging | `count` passed to `LIMIT` | `axa-cursor-pagination` |
| Database | One trait over Postgres, SQLite, MySQL, SQL in dialect constants | One crate per access library, Postgres for `axa-sqlx` |
| Migrations | Always at connect | `DB_AUTO_MIGRATE`, `db migrate` commands, pending and revert |
| SQL lint | Debug-only panic on `*` and `ALTER` | None |
| Secrets | `db_password_file` | Environment variables only |
| Configuration | TOML file plus `ATUIN_*`, writes a file on first run | Environment only, production refuses unsafe values |
| Health | `/healthz`, asks nothing | Live, ready, and startup, readiness checks the database |
| Metrics | Request count and duration, own listener on loopback | Registry rendering on the main port, no request metrics in the Prometheus crate |
| Client address | Trusted proxies, right to left, header name configurable | `TRUSTED_PROXIES`, `X-Forwarded-For` only |
| Version negotiation | Capability set hash, `412` when enforced | `Accepts-version` header, `406` problem |
| Shutdown | Graceful, no time limit | `SHUTDOWN_TIMEOUT_SECONDS` |
| Admin | User create, set password, delete | CLI with tasks and `db` commands, no user commands |
| Testing | Real server on a port, live Postgres, fixed sleep | In-process request helpers |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| Prometheus request metrics layer | Count and duration by route template is the first graph of any service | S | High |
| Secrets read from files | Container and Kubernetes secrets arrive as files, and keeping them out of the environment keeps them out of `/proc` and crash dumps | S | Medium |
| Capped blocking pool | Password hashing and other CPU-heavy work must not take every blocking thread | S | Medium |
| Optional internal listener for metrics and health | Keeps scrapes off the public port without relying on a proxy rule | M | Medium |
| Debug-time SQL lint for `axa-sqlx` | Catches `SELECT *` and `ALTER` outside migrations early | S | Low |
| Clearing the statement cache after migrating | Avoids stale prepared statements after a schema change at boot | S | Low |
| Minimum database version check | A clear boot error instead of an obscure SQL one | S | Low |
| Capability negotiation | Lets long-lived clients learn what an older server supports | M | Low |

**Prometheus request metrics.** The same proposal as in the
[Sentry Relay study](../sentry-relay/index.md): `axa_prometheus_metrics::RequestMetrics`,
registered in the service's `Registry`, with a middleware labelling by method,
`MatchedPath` template, and status. Atuin shows the trap to avoid: its unmatched
requests are labelled with the raw path, so a scanner can create a label per URL. axa
should use the fixed label `unmatched`.

**Secrets from files.** In axa's configuration, accept `DB_PASSWORD_FILE` (and, in
general, `<NAME>_FILE` for each secret-like variable that `redact` already knows), read
once at boot. Follow Atuin's refusals, which fit P-3: both the variable and its file
set is an error, an empty file is an error, a password file with a URL that already
holds a password is an error, and a trailing newline is trimmed. A percent sign in the
password must be escaped before it goes into the URL, as Atuin does. No new dependency.

**Capped blocking pool.** A small `axa::blocking::Pool` with `new(max)` and
`async fn run(f)`, a semaphore whose permit is moved into the `spawn_blocking` closure so
that it is released when the work ends, not when the caller stops waiting. A panic in the
closure is resumed in the caller. Services with local passwords, image work, or
compression of large exports would use it. It needs only Tokio.

**Internal listener.** The same proposal as in the Relay study: an `INTERNAL_PORT`
that, when set, serves the metrics path and the health routes on that port only. Atuin
binds its metrics listener to loopback by default, which is the safe default to copy.

## What axa should not copy

- **One trait over three databases with SQL in constants.** axa's spec rules out a
  database abstraction, and the dialect constants hide each query from the code that
  runs it.
- **Migrating on every connect with no way to turn it off.** axa's `DB_AUTO_MIGRATE` and
  `db migrate` commands let a service that shares a database leave it alone.
- **A health route that asks nothing.** axa's readiness, which asks the database, is
  better for a service that cannot work without it.
- **Telling a caller whether a username exists, and no login throttling.** axa's
  `lock_out_failures` and a single `401` for both cases are the safer pattern.
- **A configuration file written on first run.** axa's spec rules out configuration
  files.
- **The Kubernetes service-link guard.** It exists because Atuin reads `ATUIN_PORT`.
  axa reads `PORT` and no `<name>_PORT` variable, so a Service link does not collide
  with it.
- **Awaiting a webhook inside the request.** axa's outbox is the right place for work
  that may fail after the answer is decided.
- **Testing against a real port with a fixed sleep.** `axa-test` runs requests in process
  and needs no wait.
- **`X-Clacks-Overhead`, the server version header, and the discworld index.** These are
  Atuin's own.
