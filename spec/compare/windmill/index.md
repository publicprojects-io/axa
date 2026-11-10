# Windmill

Windmill is an open-source workflow engine and developer platform: it runs scripts, flows, and
apps written by its users on a fleet of workers. Its repository is
<https://github.com/windmill-labs/windmill>. I read the `backend/` directory at commit
[`466885f`](https://github.com/windmill-labs/windmill/tree/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend)
(10 November 2026). The repository had about 18,500 stars. The backend is a Cargo workspace of
about 880 Rust files in some 60 crates. `windmill-api` alone has 138 source files, and its HTTP
routes are spread over many `windmill-api-*` crates. It uses Axum 0.8, tower-http 0.6 (with only
the `trace`, `cors`, and `catch-panic` features), sqlx on Postgres from a fork pinned by git
revision, and Tokio. One binary runs as a server, a worker, an indexer, an agent, or all of these
together (`MODE=standalone`). The API is one large Axum `Router` built in a single function.

Links below point at that commit.

## How it is built

### App state and routing

`run_server` in [`windmill-api/src/lib.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/lib.rs#L408)
builds the whole API. There is no typed state. The pool, a `UserDB` wrapper, an auth cache, and
search index readers go in as `Extension` layers in a `ServiceBuilder` (line 447). Each feature
module exposes `workspaced_service()` or `global_service()` returning a `Router`, and these are
`nest`ed under `/api/w/{workspace_id}/...` and `/api/...`. Authentication is applied by
`route_layer(from_extractor::<ApiAuthed>())` on the authenticated group, and unauthenticated
groups (`*_u` paths) come after it. Many optional parts are switched by Cargo features inside
the router expression, with `#[cfg(not(feature = ...))] Router::new()` as the fallback. Unknown
paths fall through to an embedded single-page frontend (`fallback(static_assets::static_handler)`,
line 1244).

### Middleware

From the outside in, as added in `run_server`:

| Layer | Source |
|---|---|
| `CatchPanicLayer::custom`, answering a plain-text `500` | `lib.rs` line 1291 |
| Client and deploy-origin tagging (`set_request_client`, `set_deploy_origin`) | `lib.rs` around line 1285 |
| A task-local `LogContext` per request | [`windmill-api/src/tracing_init.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/tracing_init.rs) |
| An optional host filter for a public-app domain | `windmill-api/src/public_app_layer.rs` |
| `TraceLayer` with custom span and response logging, unless `DISABLE_RESPONSE_LOGS` | `lib.rs` line 1262 |
| Cookies, extensions, and `DefaultBodyLimit` from `REQUEST_SIZE_LIMIT` | `lib.rs` line 447 |
| Two CORS layers, `allow_origin(Any)` without credentials, put on chosen sub-routers only | `lib.rs` lines 466 and 483 |

There is no request timeout layer, no compression, and no security header layer on API
responses. The static frontend handler sets `Cache-Control` by content type, an optional
`CSP_POLICY`, `X-Robots-Tag` on login pages, and cross-origin isolation headers where needed
([`windmill-api/src/static_assets.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/static_assets.rs)).
The span takes its id from a configurable `TRACING_HEADER` (default `x-tracing-id`) or a new
UUID, logs a redacted URI, and leaves `username`, `email`, and `workspace_id` empty to be filled
once auth has run. The span is opened at `ERROR` level on purpose, so that it survives
`RUST_LOG=warn` (`tracing_init.rs`, `MyMakeSpan`).

### Errors

One `Error` enum of about 35 variants lives in
[`windmill-common/src/error.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/error.rs).
`From` impls use `#[track_caller]` to record the file of the call site. `IntoResponse` answers
`text/plain` with the error's display text, and logs `404` and `409` at `warn` and everything
else at `error`. All sqlx errors, constraint violations included, map to `400`. The `From<sqlx::Error>`
impl also spots Postgres error `0A000` ("cached plan must not change result type") and retires
every pooled connection opened before now, so that a schema change during a rolling deploy costs
one failed query rather than a failure per connection
([`windmill-common/src/db.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/db.rs#L308),
`is_stale_cached_plan`, `retire_connections_opened_before_now`, `connection_is_retired`).

### Configuration

Configuration is read from environment variables, mostly through `lazy_static` globals scattered
across modules, such as `LOGIN_RATE_LIMIT_PER_IP` in `login_rate_limit.rs` or
`SILENCE_HEALTH_LOGS` in `health.rs`. `DATABASE_URL_FILE` reads the database URL from a file,
such as a mounted secret, and wins over `DATABASE_URL`
([`windmill-common/src/lib.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/lib.rs#L2466)).
Instance settings are also stored in a `global_settings` table and edited from the UI
(`windmill-common/src/global_settings.rs`). A change to some of them makes every process restart
itself after a coordinated delay (`spawn_graceful_killpill` in
[`src/main.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/src/main.rs#L2386)).

### Authentication and authorisation

`ApiAuthed` is an extractor in
[`windmill-api-auth/src/lib.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api-auth/src/lib.rs).
It accepts a bearer header, a session cookie, or a `token` query parameter, and caches the
resolved identity in a `quick_cache` of 300 entries. API tokens are stored as a hash with a
short prefix for display (`create_token_internal`, `hash_token`). Tokens can carry scopes of the
form `domain:action[:resource]`, such as `scripts:write:f/folder/*`
([`windmill-common/src/scopes.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/scopes.rs)).
SAML, OIDC, SCIM, and OAuth login are present as `_oss` stubs whose real code is in the
enterprise edition, which I could not read.

Data access is also bounded by Postgres row-level security. `UserDB::begin` opens a transaction
and calls a SQL function `set_session_context` with the user's name, admin flag, groups, and
readable and writable folders, so that the policies in the migrations filter every query in that
transaction (`windmill-common/src/db.rs`, `UserDB::begin`; for example
`migrations/20241006144414_admin_policy.up.sql`).

[`windmill-api/src/csrf.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/csrf.rs)
has `CrossSiteGetGuard`, an extractor that marks a `GET` as cross-site when it carries
`Sec-Fetch-Site: cross-site`, or, when that header is absent, a `Referer` whose host is none of
`Host`, `X-Forwarded-Host`, or `BASE_URL`, and authenticates with the session cookie alone. Only
one dangerous kind of run refuses such a request.

### Rate limiting

`PerMinuteCounter` is an in-process fixed-window counter keyed by any hashable key, swept as calls
come in
([`windmill-common/src/per_minute_counter.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/per_minute_counter.rs)).
It backs a login limit (global, per IP, per account) in `login_rate_limit.rs` and a per-workspace
limit on public app runs in `windmill-api/src/public_app_rate_limit.rs`. The client IP is the
first entry of `X-Forwarded-For` or `X-Real-IP`, with no list of trusted proxies. The threat model
says there is no general rate limiting ([`THREAT_MODEL.md`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/THREAT_MODEL.md), T18).

### Outbound requests and SSRF

[`windmill-common/src/ssrf.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/ssrf.rs)
checks a URL that a user supplied before the server fetches it. `validate_url_for_ssrf` allows
only `http` and `https`, resolves the host once, refuses private, loopback, link-local, and
metadata addresses (`is_private_ip`), and returns a `ValidatedTarget` holding the addresses it
checked. `apply_dns_pinning` pins them on a `reqwest::ClientBuilder`, so a DNS rebinding answer
cannot change the address between check and connect. Each use (webhooks, SAML metadata, JWKS, MCP
servers, git remotes) has its own `ALLOW_PRIVATE_*` opt-in. The errors say which kind of failure
happened, so the "set this variable" hint is given only when it would help.

### Inbound webhook signatures

[`windmill-trigger-http/src/http_trigger_auth.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-trigger-http/src/http_trigger_auth.rs)
verifies HMAC signatures for GitHub, Stripe, Slack, Twitch, Zoom, and others, with SHA-1,
SHA-256, or SHA-512, hex or base64, a constant-time comparison (`constant_time_eq`), a timestamp
window of five minutes for providers that sign one, and the providers' URL-verification
challenges.

### Secrets at rest and in logs

[`windmill-common/src/workspace_crypt.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/workspace_crypt.rs)
encrypts secret variables with AES-256-GCM under an HKDF-derived key, writes a 4-byte magic
prefix (`wmA2`) to tell the format apart, and still reads older AES-CBC values. It is switched on
only once every worker reports a version that can read it (`min_version.rs`). Job logs are scanned
with an Aho-Corasick automaton for every secret a running job fetched, and matches are replaced
(`windmill-common/src/sensitive_log_masks.rs`).

### Background work and the queue

The queue is Postgres tables worked by the same binary in worker mode (`windmill-queue`,
`windmill-worker`). I did not read those crates in depth: they run user code in many languages
and are the product, not plumbing. Cross-process cache invalidation and setting changes use a
`notify_event` table that every process polls by id, instead of `LISTEN/NOTIFY`
([`windmill-common/src/notify_events.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/notify_events.rs)).

### Database and migrations

Queries are `sqlx::query!` macros with offline data in `backend/.sqlx`, and there are about 1,430
migration files in `backend/migrations`. Only servers migrate. `CustomMigrator` in
[`windmill-api/src/db.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/db.rs#L178)
wraps sqlx's migrator, takes a per-database advisory lock with `pg_try_advisory_lock` in a retry
loop with a 5-second timeout on each try, and logs why it is waiting. Workers do not migrate:
`wait_for_migrations` (line 471) polls `_sqlx_migrations` for the newest version the binary knows,
every 3 seconds, and exits after 10 tries. Some long index builds run after boot as "live
migrations" with `statement_timeout` lifted (`windmill-api/src/live_migrations.rs`). The comment
there says the pool has a 5-minute `statement_timeout`; I did not find where the pool is built.

### Observability

Prometheus gauges and OpenTelemetry are optional features. The OpenTelemetry export is an
enterprise stub (`otel_oss.rs`). `LogContext` is a task-local `ArcSwap` that carries method, URI,
trace id, user, and workspace from the request or job to every log line, so that exported logs
can be filtered by tenant without joining against traces
([`windmill-common/src/log_context.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-common/src/log_context.rs)).

### Health

[`windmill-api/src/health.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/health.rs)
serves `/api/health/status` unauthenticated. A background loop checks every 10 seconds, the answer
is cached for 5 seconds, and `?force=true` skips the cache. It reports healthy, degraded (database
up, no live workers), or unhealthy (`503`), with database latency. `/api/health/detailed` needs
authentication and adds pool statistics and worker detail. There is no separate liveness,
readiness, or startup route. An authenticated `/api/db_health` reports database size, dead-tuple
ratio per table, slow queries from `pg_stat_statements` when that extension is present, and a
recommended `max_connections` for the fleet
([`windmill-api/src/db_health.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/db_health.rs)).

### Graceful shutdown

A `KillpillSender` broadcast channel is fired by `SIGTERM`, Ctrl-C, or an internal request
(`shutdown_signal` in `windmill-common/src/lib.rs`). Shutdown has two phases: workers are told
first, and the server gets a second signal once they have stopped (`killpill_phase2_tx` in
`src/main.rs`). The server then waits for background workspace forks to finish
(`run_server`, after `with_graceful_shutdown`). A server writes a "started" row to the database
so that a peer restarting for a setting change can wait until another server is up
(`announce_server_started`).

### Server-sent events

Job progress is streamed by hand: a spawned task polls the database, skips an update whose hash
has not changed, and sends `data: ...\n\n` frames down an `mpsc` channel turned into a
`Body::from_stream`, with `Content-Type: text/event-stream`, `Cache-Control: no-cache`, and
`X-Accel-Buffering: no` (`get_job_update_sse` in
[`windmill-api/src/jobs.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-api/src/jobs.rs#L11152)).
It does not use Axum's `Sse` type and sends no keep-alive comments.

### OpenAPI

The spec is a hand-written `windmill-api/openapi.yaml`, dereferenced at build time and served
from `include_str!` at `/api/openapi.yaml` and `/api/openapi.json` (`lib.rs`, `openapi` and
`openapi_json`). I found no test that compares it with the routes, though I did not search every
crate.

### Testing

`windmill-test-utils` starts a real `ApiServer` on a random port against a pool given by
`#[sqlx::test(migrations = "../migrations", fixtures("base"))]`, so every test gets its own
migrated database with SQL fixtures, and tests talk to it with `reqwest`. It also has builders to
push a job and run a test worker until it completes
([`windmill-test-utils/src/lib.rs`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/windmill-test-utils/src/lib.rs);
for example `windmill-api-integration-tests/tests/folders.rs`). The cache module has a
`scoped_cache` feature that makes global caches thread-local for tests (`windmill-common/src/cache.rs`).

### Security documentation

[`THREAT_MODEL.md`](https://github.com/windmill-labs/windmill/blob/466885fd26b74bd20de67ee0ef0e55a47826cd47/backend/THREAT_MODEL.md)
lists 22 threats with the advisories behind each. SSRF, broken authorisation, and missing rate
limits are among the top ones, and it recommends one SSRF-guarded outbound client and one shared
webhook-signature helper.

## Compared with axa

| Area | Windmill | axa |
|---|---|---|
| Routing | One large `Router` with `nest` and `Extension` state, Cargo features inside it | `Routes` and `AppRoutes`, route census with `mounted`, per-route timeouts |
| Middleware | Panic catch, trace, body limit, CORS on chosen routes. No timeout, compression, or security headers on the API | Full standard stack, including request id, security headers, timeout, `304`, compression |
| Request id | `x-tracing-id` or a UUID, in the span only | `x-request-id`, cleaned or minted, returned on every response |
| Errors | Plain-text body, every sqlx error is `400` | RFC 9457 problem details, constraint violations `409` or `422` |
| Stale prepared statements after a schema change | Detects `0A000` and retires old pool connections | Not handled |
| Configuration | Scattered `lazy_static` env reads, plus settings in the database; `DATABASE_URL_FILE` | One `Config` from the environment, production refuses unsafe values; no `_FILE` variables |
| Auth | Bearer, cookie, or query token; hashed API tokens with scopes; Postgres RLS per transaction | Keycloak and PASETO with ABAC in the application; web sessions with CSRF tokens |
| Cross-site request check | `Sec-Fetch-Site` and `Referer` guard on one kind of `GET` | Session CSRF tokens for forms; no Fetch Metadata check |
| Rate limits | In-process per-minute counters for login and public apps; client IP from the first `X-Forwarded-For` entry | In-process request limits and failed-login lockout; client IP from trusted proxies only |
| Outbound SSRF guard | URL check with DNS pinning, per-use opt-ins | None |
| Inbound webhook signatures | HMAC for many providers, timestamp window, constant time | None |
| Encryption at rest | AES-256-GCM secret variables with a versioned prefix | Outbox payloads sealed under keyed secrets (`outbox-encryption`) only |
| Health | One cached status route with a degraded state, an authenticated detailed route, database diagnostics | Live, ready, and startup routes with named checks and reused answers |
| Migrations | Advisory lock with logged waits; workers wait for the schema | `db migrate` CLI and migrate at boot outside production; sqlx's own lock; nothing waits for the schema |
| Job queue | Product-specific, runs user code | Postgres, MySQL, and SQLite queues with schedules, retries, audited cancel and retry |
| Cross-process events | Polled `notify_event` table | Transactional outbox and relay |
| SSE | Hand-built polling stream | No helper |
| Logging context | Task-local context with user and workspace on every log line | Request id, method, and route template on the span; `axa-tracing` adds redaction and propagation |
| OpenAPI | Hand-written YAML, no drift check | Generated documents with drift checks |
| Tests | Real server per test on a `#[sqlx::test]` database with fixtures | `axa-test` boots the full router in process; no per-test database |
| Shutdown | Two-phase killpill, workers before server | Graceful shutdown of requests and jobs with a timeout |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| SSRF-guarded outbound HTTP client with DNS pinning | Any service that fetches a URL a user gave (webhooks, avatars, JWKS) needs it, and doing it without pinning is a known trap | M | High |
| Inbound webhook signature verification | Almost every service receives signed webhooks, and each one rewrites HMAC, timestamp, and constant-time checks | S | High |
| Recovery from stale prepared statements after a migration | A rolling deploy that changes a column type makes every pooled connection fail once; this costs one query instead | S | Medium |
| Postgres row-level security context per transaction | Lets a service enforce tenant or owner isolation in the database as well as in ABAC, with visible SQL | S | Medium |
| Readiness that waits for the schema | In production axa does not migrate at boot, so a new pod can serve against an old schema | S | Medium |
| Fetch Metadata check against cross-site requests | A cheap layer that refuses cross-site state-changing requests that carry cookies, which session CSRF tokens only cover for forms | S | Medium |
| Per-test database in `axa-test` | Lets tests run in parallel on isolated, migrated databases with fixtures | S | Medium |
| Server-sent events helper | SSE through axa's stack needs the timeout, compression, and buffering handled, and a clean end at shutdown | M | Medium |
| `_FILE` variants for secret settings | Kubernetes and Docker secrets are mounted as files | S | Low |
| Column encryption with key ids | The outbox already seals payloads; a service storing third-party tokens needs the same for a column | M | Low |
| User and tenant fields on the request span | Lets logs be filtered by who made the request; must stay opt-in, as it is personal data | S | Low |
| Database diagnostics for operators | Dead tuples, slow queries, and pool sizing help, but belong in a monitoring tool more than a library | M | Low |

**SSRF-guarded outbound client.** A new crate, say `axa-outbound-http`, on `reqwest` and
`hickory` or the system resolver. The shape would be
`let target = axa_outbound_http::check(&url, &Policy::default()).await?;` returning a
`Checked` value holding the URL and its resolved addresses, then
`target.client(builder)` to pin them with `reqwest::ClientBuilder::resolve_to_addrs`, with
redirects off by default. `Policy` lists allowed schemes, whether private ranges are allowed, and
extra denied CIDR ranges, reusing the `ProxyNet` parsing that `axa::client` already has for
`TRUSTED_PROXIES`. Errors would say which check failed, as Windmill's do. It should state plainly
that pinning does not reach through an egress proxy.

**Webhook signature verification.** A small crate, say `axa-webhook-signature`, on `hmac`,
`sha2`, and `subtle`. A plain function `verify(&headers, &body, &Scheme, secret) -> Result<(), Rejected>`,
with `Scheme` values for Standard Webhooks, GitHub, Stripe, and Slack, and a `Scheme::custom`
taking the header name, algorithm, encoding, and signed-payload format. It takes the raw body as
`Bytes`, so a handler reads it before parsing. A rejection would map to `401` through `axa::Error`.
Replay protection beyond the timestamp window could use the idempotency store, keyed by the
webhook id.

**Stale prepared statements.** In `axa-sqlx`: an `is_stale_cached_plan(&sqlx::Error)` like the
existing `is_unique_violation`, and a `PoolConfig` option that installs a `before_acquire` hook
closing connections opened before the last such error. `axa::Error`'s `From<sqlx::Error>` could
call it when that feature is on. This depends only on sqlx.

**Row-level security context.** In `axa-sqlx`, a function such as
`begin_with(pool, &[("app.user_id", "42"), ("app.tenant", "acme")])` that opens a transaction and
runs `SELECT set_config($1, $2, true)` for each pair, so the values last only for the transaction
and are bound, not formatted. The policies stay in the service's own migrations. This fits
"a database abstraction" being out of scope, because it adds no query layer, only a documented
way to start a transaction.

**Readiness that waits for the schema.** `axa-sqlx` already has `pending`. The readiness route
could fail, naming the database, while `pending` is not empty, behind a `Defaults` flag, and a
worker pool could wait for it before taking jobs. That gives the same guarantee as Windmill's
`wait_for_migrations` without a separate polling loop.

**Fetch Metadata check.** A layer in `axa::middleware`, off by default, that refuses a
non-safe method whose `Sec-Fetch-Site` is `cross-site` (and optionally `same-site`) when the
request carries a cookie, answering `403` as a problem. A route list can exempt webhook receivers.
Unlike Windmill's guard, it should not fall back to `Referer`, which the code itself calls weak.

**Per-test database.** In `axa-test`, a `request_with_pool` that takes the `PgPool` handed to a
`#[sqlx::test]` function and boots `TestApp` with it, so fixtures and isolation come from sqlx.
It depends on nothing new.

**Server-sent events.** A module `axa::sse` with a function that turns a `Stream` of
serialisable items into Axum's `Sse` with a keep-alive, sets `X-Accel-Buffering: no`, and ends
the stream when the shutdown signal fires. The route would be declared with no timeout through
`Routes::add_with_timeout`, and the compression layer would need to skip `text/event-stream`,
which should be checked and tested.

## What axa should not copy

- **Configuration in scattered globals and in the database.** `lazy_static` env reads in each
  module hide what a deployment runs with, which is the opposite of axa's single `Config`.
  Settings edited in a database table and a restart on change are a product feature for a
  self-hosted admin UI.
- **Every sqlx error as `400`, and plain-text errors.** axa's problem details and constraint
  mapping are better.
- **Tokens in a query parameter.** Windmill needs it for `EventSource` and webhook URLs, and its
  own CSRF code has to work around it. A library should not offer it by default.
- **Client IP from the first `X-Forwarded-For` entry.** The caller chooses that entry. axa's
  trusted-proxy rule is already correct.
- **`CorsLayer` with `allow_origin(Any)` on chosen routes.** Fine for Windmill's embeds, but axa's
  "none until listed, `*` refused in production" is the safer default.
- **The polled `notify_event` table.** It suits a fleet that cannot rely on `LISTEN/NOTIFY` through
  a pooler, but axa's outbox already covers reliable messages between processes.
- **Version gates for the worker fleet (`min_version.rs`).** They exist because Windmill workers
  run user code from many releases at once. They are tied to its release numbering.
- **Secret masking in job logs.** It matches secrets inside the output of user code. axa's
  `Redacted` field type is the right tool for a service's own logs.
- **Cargo features inside the router expression and `_oss`/`_ee` file pairs.** They reflect an
  open-core build, and make the router hard to read. axa's crates per feature are clearer.
- **A hand-written OpenAPI file with no drift check.** axa already does better.
- **One health route with a "degraded" state.** axa's three probes are the better model for
  Kubernetes. The detailed and database routes are operator tooling, and a service can add them
  as its own authenticated routes.
