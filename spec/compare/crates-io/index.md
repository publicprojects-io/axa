# crates.io

crates.io is the Rust package registry: the web API behind `cargo publish`, `cargo
search`, owners, yanking, API tokens, and Trusted Publishing, plus a SvelteKit frontend.
Its repository is [github.com/rust-lang/crates.io](https://github.com/rust-lang/crates.io).
I read the `main` branch at commit
[`4260331`](https://github.com/rust-lang/crates.io/tree/426033177301a42884b592917a3c5e146ef18d11)
of 9 November 2026. It has about 3,700 stars, is licensed Apache-2.0 (and MIT), and its
tree holds about 530 Rust files, 28 workspace crates under `crates/`, and 293 Diesel
migrations. It runs on Heroku as three processes from one binary: `migrate` on release,
`server`, and `background-worker` ([`Procfile`][procfile]). The server is Axum 0.8 with
`tower-http` 0.7, Diesel 2.3 through `diesel-async` and `deadpool`, `utoipa` for OpenAPI,
and OpenTelemetry for traces and metrics ([`Cargo.toml`][cargo]).

> **Since this was written**, axa 0.11 added a database that cannot serve answers `503` with `Retry-After`. Those rows below are marked "Done in 0.11", and the rest stand as written.

Links below are to files at that commit, under
`https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/`.

[procfile]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/Procfile
[cargo]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/Cargo.toml
[main]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/bin/crates-io/main.rs
[serverbin]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/bin/crates-io/server.rs
[workerbin]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/bin/crates-io/background_worker.rs
[admin]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/bin/crates-io/admin/mod.rs
[enqueue]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/bin/crates-io/admin/enqueue_job.rs
[migrate]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/bin/crates-io/admin/migrate.rs
[server]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/server.rs
[router]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/router.rs
[mw]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware.rs
[realip]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/real_ip.rs
[block]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/block_traffic.rs
[blockcfg]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/config/block.rs
[ua]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/require_user_agent.rs
[norm]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/normalize_path.rs
[log]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/log_request.rs
[cdnauth]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/cdn_auth.rs
[common]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/common_headers.rs
[compat]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/cargo_compat.rs
[updmetrics]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/middleware/update_metrics.rs
[errors]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/util/errors.rs
[errjson]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/util/errors/json.rs
[dieselutil]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/util/diesel.rs
[db]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/db.rs
[pools]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/config/database_pools.rs
[features]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/config/features.rs
[envvars]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_env_vars/src/lib.rs
[ratelimiter]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/rate_limiter.rs
[ratecfg]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/config/rate_limits.rs
[auth]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/auth.rs
[token]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_database/src/utils/token.rs
[tokenmodel]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_database/src/models/token.rs
[scan]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/controllers/github/secret_scanning.rs
[origin]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/controllers/util.rs
[session]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_session/src/lib.rs
[encryption]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_encryption/src/lib.rs
[pagination]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/controllers/helpers/pagination.rs
[openapi]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/openapi.rs
[apidesign]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/docs/API-DESIGN.md
[worker]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_worker/README.md
[bgjob]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_worker/src/background_job.rs
[storagejobs]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_worker/src/storage.rs
[listener]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_worker/src/listener.rs
[runner]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_worker/src/runner.rs
[collector]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/metrics/collector.rs
[smetrics]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/metrics/server.rs
[tracing]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/util/tracing.rs
[email]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/email.rs
[storage]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/storage.rs
[testdb]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/crates/crates_io_test_db/src/lib.rs
[nextest]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/.config/nextest.toml
[testapp]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/tests/util/test_app.rs
[chaos]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/src/tests/util/chaosproxy.rs
[guard]: https://github.com/rust-lang/crates.io/blob/426033177301a42884b592917a3c5e146ef18d11/diesel-guard.toml

## How it is built

### Process and startup

[`main.rs`][main] is a `clap` enum of `server`, `background-worker`, and about twenty
admin commands ([`admin/mod.rs`][admin]), such as `migrate`, `delete-crate`,
`yank-version`, `verify-token`, and `enqueue-job`. Each admin command runs inside an
`admin.command` span whose OpenTelemetry status is set from the result. Configuration is
read once from the environment into `SharedConfig`, through small helpers (`var`,
`required_var`, `var_parsed`, `var_bool`, `list`) in
[`crates_io_env_vars`][envvars]. There are no configuration files.

[`server.rs`][serverbin] builds a Tokio runtime with four worker threads and an optional
`max_blocking_threads`, serves with `into_make_service_with_connect_info`, and shuts down
gracefully on `SIGINT` or `SIGTERM`. There is no drain timeout beyond what
`axum::serve` does. The `migrate` command ([`migrate.rs`][migrate]) runs embedded Diesel
migrations and then syncs a TOML list of categories. In read-only mode it skips both and
exits with success, so that Heroku can still release a config change during an outage.

### App state and routing

`ServerContext` ([`server.rs`][server]) is an `Arc` around a struct built with the `bon`
typed builder: primary pool, optional replica pool, GitHub client, OAuth client, OIDC key
stores, config, email, object storage, metrics, and the rate limiter. It derives
`FromRequestParts` via `State`, so handlers take it directly.

[`router.rs`][router] registers almost every route through `utoipa_axum`'s `routes!`
macro on an `OpenApiRouter`, then `split_for_parts` yields the router and the spec, which
is served at `/api/openapi.json`. Two private routes sit outside the spec. The fallback
answers `404` with an empty body to `HEAD` and a JSON error otherwise. A path
normalisation layer ([`normalize_path.rs`][norm]) wraps the whole router from outside,
collapsing `//`, `/.`, and `..`, and keeps the original path for the access log.

### Middleware

[`middleware.rs`][mw] splits the stack in two `ServiceBuilder`s to keep compile times
down, with a comment linking the PR. From the outside in:

| Layer | What it does |
|---|---|
| `TraceLayer` | Span per request, with OpenTelemetry fields |
| Compression | `CompressionLevel::Fastest` |
| `RequestBodyTimeoutLayer` | 30 seconds to read the body |
| `TimeoutLayer` | `408` after 30 seconds, one value for every route |
| Real IP ([`real_ip.rs`][realip]) | Takes the address from `X-Forwarded-For` using `crates_io_real_ip`, which trusts only known CDN ranges, else the socket address |
| Access log ([`log_request.rs`][log]) | One line per request in a Heroku-like format, with the client IP, route template, user agent, request id, a SHA-256 of the `Authorization` and `Cookie` headers, error kind and message, and free-form fields a handler adds through a `RequestLog` extension |
| `CatchPanicLayer` | Panic becomes `500` |
| Active-request gauge ([`update_metrics.rs`][updmetrics]) | OpenTelemetry up-down counter by method and scheme |
| CDN origin auth ([`cdn_auth.rs`][cdnauth]) | When a token is set, refuses any request without the shared secret header the CDN adds, so the origin cannot be hit directly |
| Cargo compatibility ([`cargo_compat.rs`][compat]) | Rewrites Axum's plain-text extractor rejections as JSON errors, and adjusts status codes for old `cargo` clients |
| Session | Signed cookie session ([`crates_io_session`][session]) |
| Require user agent ([`require_user_agent.rs`][ua]) | `403` without a `User-Agent`, except on download routes |
| Block traffic ([`block_traffic.rs`][block]) | Refuses by IP (`BLOCKED_IPS`), by header value or regex (`BLOCKED_TRAFFIC`), and by route template (`BLOCKED_ROUTES`, answering `503`) |
| Common headers ([`common_headers.rs`][common]) | HSTS, `nosniff`, `X-Frame-Options`, CORS `*`, and `Cache-Control` for a fixed list of static paths |
| Frontend | Serves the SvelteKit build and HTML shell when configured |

### Errors

[`errors.rs`][errors] defines `AppError`, a trait with one method, `response()`, and
`BoxedAppError = Box<dyn AppError>`. Any `std::error::Error` becomes a `500` whose
source chain is logged through an `ErrorField` response extension, not sent to the
caller. The body is `{"errors":[{"detail":"..."}]}` ([`json.rs`][errjson]), not RFC 9457.
Database errors are mapped with care for outages: `NotFound` is `404`; a write refused
because the transaction is read-only ([`util/diesel.rs`][dieselutil]) is `503` with a
status-page message; a closed connection and any pool error are `503`. `TooManyRequests`
sets `Retry-After` as an HTTP date.

### Database

[`db.rs`][db] builds `deadpool` pools of `AsyncPgConnection` with TLS using a pinned root
certificate. Each new connection runs `SET application_name`, `SET statement_timeout`, and,
in read-only mode, `SET default_transaction_read_only = 't'`. `tcp_user_timeout` is added
to the URL so that full packet loss to the database fails fast.
[`database_pools.rs`][pools] reads `DATABASE_URL`, an optional `READ_ONLY_REPLICA_URL`,
`READ_ONLY_MODE`, pool sizes, and timeouts, and `DB_OFFLINE=leader` or `follower` lets an
operator route everything to the surviving database. `ServerContext` offers `db_write`,
`db_read` (replica, falling back to primary), and `db_read_prefer_primary` (the reverse),
and counts each fallback in a metric. The worker raises the statement timeout to four
hours for its long jobs ([`background_worker.rs`][workerbin]). Migrations are checked in
CI by `diesel-guard` for unsafe DDL such as `CREATE INDEX` without `CONCURRENTLY`
([`diesel-guard.toml`][guard]).

### Auth

[`auth.rs`][auth] reads a cookie session or an `Authorization` token. `AuthCheck` is a
builder a handler calls: `only_cookie()`, `with_endpoint_scope(..)`, `for_crate(..)`, and
`allow_any_crate_scope()`. Tokens carry endpoint scopes and crate-name patterns, and each
check is counted in a metric with its rejection reason. API tokens
([`utils/token.rs`][token]) are `cio` plus 32 random alphanumerics, stored only as a
SHA-256 hash; lookup filters out revoked and expired tokens and updates `last_used_at`
unless the database is read-only ([`models/token.rs`][tokenmodel]). A GitHub secret
scanning endpoint ([`secret_scanning.rs`][scan]) verifies GitHub's signed alerts and
revokes leaked tokens. Cookie-authenticated requests must carry an allowed `Origin`
([`controllers/util.rs`][origin]). OAuth tokens for GitHub are stored encrypted with
AES-256-GCM ([`crates_io_encryption`][encryption]).

### Rate limiting

[`rate_limiter.rs`][ratelimiter] is a token bucket per user and action (publish new,
publish update, yank), stored in Postgres and updated in one `INSERT ... ON CONFLICT DO
UPDATE ... RETURNING`, so every server process shares it. A `publish_rate_overrides` table
raises the burst for a user, optionally until a date. Rates and bursts come from
`RATE_LIMITER_{ACTION}_RATE_SECONDS` and `_BURST` ([`rate_limits.rs`][ratecfg]). There is
no per-IP request limit in the app; that is left to the CDN.

### Background jobs

[`crates_io_worker`][worker] is a Postgres queue in one `background_jobs` table. A job
type implements `BackgroundJob` with `JOB_NAME`, `PRIORITY`, `DEDUPLICATED`, and `QUEUE`
([`background_job.rs`][bgjob]). A deduplicated enqueue inserts only if no unlocked row
with the same type, data, and priority exists. Workers claim with `FOR UPDATE SKIP
LOCKED`, highest priority first, and a failed job waits `2^retries` minutes
([`storage.rs`][storagejobs]). A trigger sends `NOTIFY` on insert, and the runner keeps
one connection on `LISTEN` with reconnect backoff to wake workers at once, with polling as
fallback ([`listener.rs`][listener]). Named queues each get their own worker count
([`runner.rs`][runner]). Recurring work is not scheduled in-process: an external scheduler
runs `crates-io enqueue-job <name>` ([`enqueue_job.rs`][enqueue]). A collector in the
worker records queue depth and oldest job age every five seconds
([`collector.rs`][collector]).

### Observability

[`util/tracing.rs`][tracing] builds a `tracing` subscriber with compact or JSON output
(`RUST_LOG_FORMAT`), an OpenTelemetry layer, and a panic hook that logs the message and
backtrace before the default hook. Metrics are OpenTelemetry instruments exported by OTLP
([`metrics/server.rs`][smetrics]), not a Prometheus scrape endpoint: active requests, auth
checks, database fallbacks, and pool usage. There is no health route in the router; I did
not find one anywhere in `src/`. Liveness is left to the platform.

### Other

- Email ([`email.rs`][email]): `lettre` with SMTP, file, or in-memory transports,
  MiniJinja templates with text and HTML bodies, masked addresses in logs, and a panic at
  start-up if production is not on SMTP.
- Object storage ([`storage.rs`][storage]): `object_store` over S3, local disk, or memory,
  with streamed uploads and CDN cache tags.
- Pagination ([`pagination.rs`][pagination]): page numbers and opaque seek keys, refusing
  both at once. Deep page numbers are logged as a suspected bot and can be refused past a
  configured offset.
- API conventions are written down in [`docs/API-DESIGN.md`][apidesign].
- Feature flags are plain booleans in config ([`features.rs`][features]).

### Testing

[`crates_io_test_db`][testdb] migrates a `test_template` schema once, captures its DDL
with `pg_dump`, and gives each test a fresh random schema by replaying that DDL, reached
through `search_path` in the URL; the schema is dropped with the value. Under nextest a
setup script prepares the template once for the whole run ([`nextest.toml`][nextest]).
[`TestApp`][testapp] is a builder (`init()`, `with_user()`, `with_token()`,
`with_config(..)`, `with_rate_limit(..)`) with in-memory email and storage, a method to
run pending background jobs, and snapshot helpers built on `insta`. A `ChaosProxy`
([`chaosproxy.rs`][chaos]) sits between the app and Postgres so tests can cut the network
and check the `503` and recovery paths.

## Compared with axa

| Area | crates.io | axa |
|---|---|---|
| Routing | `utoipa_axum` `routes!` macro, spec from the router | `Routes`/`AppRoutes`, plain functions, OpenAPI drift checks |
| Timeouts | One 30 s timeout, plus a 30 s body-read timeout | Global, per-group, and per-route timeouts; header read timeout |
| Client address | Trusts known CDN ranges in `X-Forwarded-For` | `TRUSTED_PROXIES` list |
| Errors | `{"errors":[{"detail"}]}`, pool, closed-connection and read-only errors become `503` | RFC 9457; constraint violations to `409`/`422`; pool and connection failures fall to `500` |
| Access log | Hashed auth and cookie headers, handler-added fields | Route template and request id on every line, path and query left out |
| Operator blocks | IP, header regex, and route kill switch by env | None |
| Database pools | Primary and replica with fallback, read-only mode, `statement_timeout`, `application_name`, `tcp_user_timeout` | One pool per database; connect and idle timeouts |
| Migrations | Embedded Diesel, linted by `diesel-guard` | `db migrate` in the CLI |
| Auth | Cookie session plus hashed opaque API tokens with scopes, expiry, last use | Keycloak, PASETO, ABAC, web sessions with CSRF |
| Rate limit | Per user and action, token bucket in Postgres, overrides table | Per client or key, in process, failed-login lockout |
| Job queue | Priority, dedup, named queues, `LISTEN`/`NOTIFY`, exponential retry | Priority, delays, schedules, heartbeats, audited cancel/retry, three databases |
| Schedules | External scheduler calls `enqueue-job` | In-process schedules, once per due time across replicas |
| Metrics | OpenTelemetry OTLP | Prometheus and OpenTelemetry |
| Health | None in the app | `/_health/live`, `/ready`, `/startup` |
| Email | `lettre` with SMTP, file, memory | None |
| Object storage | `object_store` S3, disk, memory | None |
| Tests | Per-test schema from a migrated template, `TestApp` builder, chaos proxy | `TestApp` trait and request helpers |
| Idempotency, outbox, version negotiation | None found | Yes |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| Map pool timeouts, closed connections, and read-only transactions to `503` | A database outage now answers `500`, which says "our bug" and is not retried | S | Done in 0.11 (T-5, DB-7) |
| Per-connection setup: `statement_timeout`, `application_name`, optional read-only | A runaway query holds a connection for ever; `pg_stat_activity` cannot name the service | S | High |
| Per-test database schema from a migrated template in `axa-test` | Tests that share one database cannot run in parallel safely, and per-test migrations are slow | M | High |
| `LISTEN`/`NOTIFY` wake-up for `axa-postgres-job-queue` | Removes up to one poll interval of latency without polling faster | M | Medium |
| Deduplicated enqueue | Stops a burst of identical jobs, such as "reindex X", piling up | S | Medium |
| Shared token bucket in Postgres for named actions | In-process limits multiply with replicas; costly actions such as sign-up or export need one limit everywhere | M | Medium |
| Operator traffic block by IP, header, and route template | Lets an operator shut off an abusive client or a broken route by config, without a deploy | S | Medium |
| Read replica pool with fallback | Takes read load off the primary and survives a replica outage | M | Medium |
| Opaque API tokens: prefix, SHA-256 at rest, scopes, expiry, last use | A common need for machine clients, beside Keycloak and PASETO | M | Medium |
| Request-scoped log fields | Lets a handler add `cause` or `bot=suspected` to the one access line | S | Low |
| Body-read timeout | Bounds a slow upload separately from handler time | S | Low |
| Email sender with SMTP, file, and memory backends | Generic, but product templates stay in the service | M | Low |
| Object storage wrapper | `object_store` is already the library; a wrapper adds little | M | Low |
| Migration linting (`diesel-guard` or `squawk`) | A tool, not code: worth a line in the docs or CI template | S | Low |
| Chaos proxy test helper | Proves the `503` path; useful once the mapping above exists | S | Low |

**Outage errors as `503`.** This belongs in `src/error.rs` beside `broken_sqlx` and the
Diesel `From`. Map `sqlx::Error::PoolTimedOut`, `PoolClosed`, and `Io`, the Diesel pool
and `ClosedConnection` errors, SQLSTATE `25006` (read-only transaction), and probably
`57014` (statement cancelled) to a `503` problem with a stable `code` such as
`unavailable`, and no database text in it. A `Retry-After` of a few seconds is optional.
No new dependency.

**Per-connection setup.** `axa-sqlx` can use sqlx's `after_connect`, `axa-diesel` a
deadpool `post_create` hook as crates.io does, and the other database crates their
equivalents, all with the same environment names: `DB_STATEMENT_TIMEOUT_SECONDS`
(default perhaps 30), `DB_APPLICATION_NAME` (default the service name), and
`DB_READ_ONLY`. The worker can set a longer timeout for its own pool. Production could
refuse an unset statement timeout, in line with axa's refusal of unsafe settings.

**Per-test schema.** In `axa-test`, a `TestDatabase::new()` that on first use migrates a
template schema, keeps its DDL, and replays it into a fresh random schema reached by
`search_path`, dropped on `Drop`. crates.io captures the DDL with `pg_dump`, which is
fast but needs that tool on the test machine. A first version can simply run the
service's migrator into each new schema, which needs only `sqlx`, and add the template
later if it is slow. Postgres only at first.

**`LISTEN`/`NOTIFY`.** A trigger on `axa_queue` insert, created by the queue's setup SQL,
and one listening connection in `Queue::run` that wakes the loops through a
`tokio::sync::Notify`, reconnecting with backoff. The poll stays as the fallback, so a lost
notification only costs latency. MySQL and SQLite keep polling.

**Deduplicated enqueue.** `NewJob::dedup_key(key)` storing a key column with a partial
unique index `WHERE status = 'queued'`, and `INSERT ... ON CONFLICT DO NOTHING`. A key the
caller chooses is clearer than crates.io's comparison of whole JSON arguments.

**Shared action limit.** A table of buckets and one upsert, as in crates.io, in
`axa-request-rate-limit` behind a Postgres feature: `ActionLimit::new(name, rate,
burst).check(&mut tx, key)` returning the existing `Verdict` so the `429` and
`Retry-After` match the in-process limiter. Overrides can stay in the service.

**Operator block.** A layer in `axa::middleware`, off by default, reading
`BLOCKED_CLIENT_ADDRESSES`, `BLOCKED_ROUTES` (route templates, `503`), and a header rule
list, using the client address axa already resolves. It answers with a problem that
carries the request id. Config only; no regex dependency unless header patterns are
wanted.

**Read replica.** An optional second pool in each database crate (`DB_REPLICA_URL`) and a
`read()` accessor that falls back to the primary and counts it. Readiness should not fail
on the replica alone.

**Opaque API tokens.** A new crate, say `axa-api-token`: `PlainToken::generate(prefix)`,
`HashedToken::parse(prefix, text)`, a sqlx table with hash, scopes, expiry, revoked, and
last used, and an Axum layer that puts the token's subject on the request. It should work
with the existing ABAC rather than adding a second scope model.

## What axa should not copy

- **Trusted Publishing, crate scopes, the index, typosquat checks, OG images, readmes,
  docs.rs and CDN invalidation jobs.** These are the registry's product.
- **`AppError` as a boxed trait object with a custom JSON envelope.** axa's RFC 9457
  problems with stable codes are the better contract, and the envelope is kept for
  `cargo` compatibility only ([`docs/API-DESIGN.md`][apidesign]).
- **The `routes!` macro as the router.** It ties routing to code generation, which axa
  rules out. axa's drift check gets the same assurance without it.
- **No health routes.** crates.io relies on Heroku; axa's three probes are better for
  Kubernetes.
- **External scheduling through a CLI `enqueue-job`.** axa's in-process schedules, safe
  across replicas, are simpler to run.
- **Trusting `X-Forwarded-For` by CDN IP ranges, and CDN origin auth.** Both are tied to
  their CDNs (`crates_io_real_ip` and the shared secret header). axa's `TRUSTED_PROXIES`
  is the general form; an origin secret is one line a service can add.
- **`User-Agent` required.** A policy for a public API used by scripts, not a default.
- **Blanket `Access-Control-Allow-Origin: *` and static-path cache headers** in
  [`common_headers.rs`][common]. axa keeps CORS off until configured.
- **A pinned root certificate and `danger_accept_invalid_certs` outside production** in
  [`db.rs`][db]. Specific to their database host.
- **Splitting the middleware stack for compile time.** A workaround for their stack, not
  a feature.
