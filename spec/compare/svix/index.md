# Svix webhooks server

Svix is a service that sends webhooks for other services. It stores messages, fans them
out to subscriber endpoints, signs each delivery, retries failures, and records every
attempt. Its repository is
[github.com/svix/svix-webhooks](https://github.com/svix/svix-webhooks), and the server is
under `server/`. I read the `main` branch at commit
[`2a61d8f`](https://github.com/svix/svix-webhooks/tree/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server)
of 9 November 2026. The repository has about 3,500 stars and is licensed MIT. The server
has 87 Rust files, about 980 KB of source with its tests. It uses Axum 0.8 with `aide`
for code-first OpenAPI, `tower-http` for CORS, tracing, and path normalisation, SeaORM
over sqlx for Postgres, `omniqueue` for Redis, RabbitMQ, or in-memory queues, and one
binary that runs the API, the delivery worker, or both.

Links below are to files at that commit, under
`https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/`.

[lib]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/lib.rs
[main]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/main.rs
[cfg]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/cfg.rs
[v1]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/v1/mod.rs
[utils]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/v1/utils/mod.rs
[patch]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/v1/utils/patch.rs
[health]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/v1/endpoints/health.rs
[err]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/error/mod.rs
[idem]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/idempotency.rs
[sec]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/security.rs
[perm]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/permissions.rs
[crypto]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/cryptography.rs
[types]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/types/mod.rs
[client]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/webhook_http_client.rs
[cache]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/cache/mod.rs
[retry]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/retry.rs
[spans]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/otel_spans.rs
[opwh]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/core/operational_webhooks.rs
[queue]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/queue/mod.rs
[rqueue]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/queue/redis.rs
[redis]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/redis/mod.rs
[worker]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/worker.rs
[db]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/db/mod.rs
[bgm]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/db/background_migrations.rs
[clean]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/expired_message_cleaner.rs
[wait]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/wait_for.rs
[openapi]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/src/openapi.rs
[derive]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server-derive/src/aide.rs
[testutil]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/tests/it/utils/mod.rs
[cargo]: https://github.com/svix/svix-webhooks/blob/2a61d8f5979eb9f00e66b1a59cd4bca23127a88a/server/svix-server/Cargo.toml

## How it is built

### App state and start-up

[`src/lib.rs`][lib] holds `AppState`: a SeaORM `DatabaseConnection`, a queue producer,
the configuration, a cache, and a sender for "operational webhooks" (events about the
service itself). `run_with_prefix` builds these, then uses `tokio::join!` to run four
parts side by side: the API server, the delivery worker, an expired-message cleaner, and
background migrations. Each is turned on or off by configuration (`api_enabled`,
`worker_enabled`, and so on), so one binary can be an API pod, a worker pod, or both.
It can take a ready `TcpListener`, which the tests use to bind a free port.

[`src/main.rs`][main] is a `clap` CLI with `migrate`, `jwt generate`, `asymmetric-key
generate`, `wipe` of one organisation, and reverting a background migration by id. With
`--wait-for`, start-up first polls the TCP ports of Postgres, Redis, and RabbitMQ until
they answer ([`src/wait_for.rs`][wait]).

### Routing and middleware

[`src/v1/mod.rs`][v1] merges one `aide::ApiRouter` per resource and adds a
`tower_http::TraceLayer` with custom span makers. A debug build also mounts a
`/development/echo` route. `lib.rs` then nests it at `/api/v1`, adds a docs router, and
applies an idempotency service and a fully open CORS layer (`allow_origin(Any)`). It wraps
the whole app in `NormalizePath::trim_trailing_slash`, outside the router, so a trailing
slash is removed before routing. There is no request timeout, body limit, security
headers, panic catching, compression, or rate limit in the server's own stack. I found
no rate-limit code anywhere in `server/`.

### Errors and validation

[`src/error/mod.rs`][err] has one `Error` with an `ErrorType` and a `trace` of
`&'static Location`. Each constructor is `#[track_caller]`, and a `Traceable::trace()`
method pushes the caller's file and line as an error moves up, so a log shows the path
of an error without a backtrace. A 500 answers an empty `{}`. Client errors answer
`{"code": ..., "detail": ...}`, which is not RFC 9457.

[`src/v1/utils/mod.rs`][utils] has `ValidatedJson` and `ValidatedQuery`. `ValidatedJson`
deserialises with `serde_path_to_error`, so a type error names where it was, such as
`["body", "channels", "[0]"]`, then runs `validator`. Nested and list errors are
flattened into a list of `{loc, msg, type}` items, answered as `422`. The value the
caller sent is not quoted. [`src/v1/utils/patch.rs`][patch] has
`UnrequiredNullableField<T>` (absent, null, or a value) and `UnrequiredField<T>` (absent
or a value), so a `PATCH` can tell "leave it" from "clear it". Pagination is by an opaque
`iterator` and `prev_iterator`, which are entity ids (KSUIDs) and not signed.

### Configuration

[`src/cfg.rs`][cfg] uses `figment`: built-in TOML defaults, then `config.toml`, then
`SVIX_`-prefixed environment variables. The struct is checked with `validator`, plus a
whole-config check. Unsafe settings carry a `dangerous_` prefix, such as
`dangerous_disable_tls_verification`. At start-up, [`lib.rs`][lib] warns when two
settings conflict, such as a Redis pending timeout shorter than the worker's HTTP
timeout. It warns, and does not refuse.

### Auth

[`src/core/security.rs`][sec] reads a bearer JWT with `jwt-simple`. The signing key can
be HS256/384/512, RS256/384/512, or EdDSA, and a public key alone verifies tokens
without being able to mint them. If a caller sends the signing secret itself as the
token, the answer says so. Claims carry an organisation, an optional application, and
feature flags. [`src/core/permissions.rs`][perm] turns those into extractors
(`Organization`, `Application`, `ReadAll`, and others) that each handler takes.

### Idempotency

[`src/core/idempotency.rs`][idem] is a `tower::Service` around the whole app. For a
`POST` with an `idempotency-key` and an `Authorization` header, it hashes the token, key,
and URI with BLAKE2b into a cache key. It sets a "started" marker for five seconds. A
second request polls every 200 ms. If the first has not finished in five seconds, the
second runs the handler again. Only `2xx` answers are kept, for twelve hours. The request
body is not fingerprinted, so the same key with a different body gets the first answer.
It is a no-op when the cache is off.

### Queue, worker, and retries

[`src/queue/mod.rs`][queue] chooses Redis (single, cluster, or sentinel), RabbitMQ, or an
in-memory queue through `omniqueue`. [`src/queue/redis.rs`][rqueue] uses Redis streams
with a consumer group for work in progress, and a sorted set for delayed tasks, moved by
a background task under a Redis lock. It migrates older queue layouts on start.

[`src/worker.rs`][worker] spawns a task per delivery, capped by `worker_max_tasks`, and
at shutdown waits for running tasks to finish. A retry uses a configured list of delays
(`retry_schedule`), not a formula. A timeout or a `429` waits at least a fixed penalty.
A `Retry-After` header from the receiver is honoured but clamped between the scheduled
delay and twice it. Every delay gets random jitter. An endpoint that has failed for a
whole configured period with no success is disabled, and an operational webhook says so.
[`src/core/retry.rs`][retry] is a small `run_with_retries(fn, should_retry, schedule)`
for in-process calls to Redis and the queue.

### Outbound HTTP and signing

[`src/core/webhook_http_client.rs`][client] is the most reusable part. It is a
`hyper-util` client whose DNS resolver drops every address that is private, loopback,
link-local, CGNAT, documentation, benchmarking, reserved, 6to4, NAT64, and more, so a
user-supplied URL cannot reach the internal network. The check runs on the resolved
address, so a public name pointing at `10.0.0.1` is refused too, and a literal IP in the
URL is checked before connecting. Allowed subnets and host names are configured. It can
go through a SOCKS5 or HTTP proxy with a no-proxy list, has a per-request timeout, and
preserves header case.

Deliveries are signed per the Standard Webhooks scheme. `sign_msg` in
[`src/worker.rs`][worker] signs `{id}.{timestamp}.{body}` with every signing key of the
endpoint and sends them space-separated, `v1,<HMAC-SHA256>` or `v1a,<Ed25519>`. Old keys
are kept with an expiry during rotation ([`src/core/types/mod.rs`][types]), so a receiver
can switch keys without missing a delivery. Secrets have `whsec_`, `whsk_`, and `whpk_`
prefixes. [`src/core/cryptography.rs`][crypto] encrypts secrets at rest with
XChaCha20-Poly1305 under one main key.

### Database and migrations

[`src/db/mod.rs`][db] runs embedded sqlx migrations, and SeaORM for queries.
[`src/db/background_migrations.rs`][bgm] is a second kind of migration for work that
cannot run in a transaction or must not block start-up, such as `CREATE INDEX
CONCURRENTLY`. Each has an id, apply SQL, optional clean-up SQL to run before a retry,
and optional revert SQL. One replica runs them, under a Postgres advisory lock, and a
`_svix_background_migrations` table records start, finish, attempts, and the last error.
[`src/expired_message_cleaner.rs`][clean] nulls expired payloads in batches, with
`statement_timeout` lifted for the batch.

### Cache and Redis

[`src/core/cache/mod.rs`][cache] has one `Cache` over none, memory, or Redis backends.
A `kv_def!` macro binds a key type to its value type, so a key cannot be read as the
wrong type. It has `set_if_not_exists`, which the idempotency service uses as a lock.
[`src/redis/mod.rs`][redis] connects to a single Redis, a cluster, or a sentinel set
through one `RedisManager`.

### Observability

[`src/lib.rs`][lib] sets up `tracing` with stdout (plain or JSON), OTLP traces with a
sample ratio, OTLP metrics with an `instance_id`, optional OTLP logs that copy span fields
onto each record, and Sentry for warnings and errors. It filters the OTLP exporter's own
crates out of log export, so exporting cannot feed itself.
[`src/core/otel_spans.rs`][spans] builds request spans with OpenTelemetry field names and
the route template, and takes the parent from `traceparent` when OTLP is on.

### Health

[`src/v1/endpoints/health.rs`][health] has `/api/v1/health/ping`, which answers `204`
and asks nothing, and `/api/v1/health`, which runs `SELECT 1`, sends a task through the
queue, and writes a cache key, every time it is called. It answers `500` with a status
for each of the three. It has no caching and no time limit.

### OpenAPI

Handlers are documented in code with `aide`. A proc macro, `#[aide_annotate]` in
[`svix-server-derive/src/aide.rs`][derive], takes a handler's doc comment as the
operation description, makes a summary from its name, and refuses a handler with no doc
comment at compile time. [`src/openapi.rs`][openapi] post-processes the document: it adds
an `idempotency-key` header to every `POST`, tag groups, and `x-webhooks` entries from the
operational-webhook payload types. The `/api/v1/openapi.json` route serves a checked-in
file, and a separate `generate-openapi` binary writes it.

### Testing

[`tests/it/utils/mod.rs`][testutil] starts the real server on a free port per test, with
its own queue prefix so tests do not take each other's tasks, and gives a `TestClient`
whose methods take the expected status. A `TestReceiver` is a small Axum server on a
free port that records each request it receives and answers with a status the test can
change at run time, used to test deliveries end to end. `wiremock` is also a dev
dependency ([`Cargo.toml`][cargo]).

## Compared with Axa

| Area | Svix | Axa |
|---|---|---|
| App shape | One binary, API and worker roles by config | `service start`, `--worker`, `--all`; library, not a binary |
| Middleware | CORS open to all, trace, trailing-slash trim | Full stack: request id, trace, panic, security headers, CORS off by default, timeout, 304, compression, body limit |
| Errors | `{code, detail}`, `#[track_caller]` trace of locations | RFC 9457 problems, constraint errors mapped to `409`/`422` |
| JSON errors | Field path via `serde_path_to_error`, `{loc, msg, type}` list | Fixed text that quotes nothing; no field path for a type error |
| PATCH | Tri-state absent/null/value types | Not found |
| Config | figment TOML and env, warns on conflicts | Env, production refuses unsafe settings |
| Auth | JWT with HS/RS/EdDSA, claims to extractors | PASETO and Keycloak with ABAC |
| Idempotency | Redis, no body fingerprint, reruns after 5 s, keeps `2xx` only | Postgres, fingerprinted (`422` on a different body), `409` while in progress |
| Queue | Redis streams, RabbitMQ, memory | Postgres, MySQL, SQLite, with audited cancel/retry |
| Retry delay | Schedule list, jitter, `Retry-After` clamp, overload penalty | Doubling from a base, capped at an hour, no jitter |
| Circuit breaking | Disables an endpoint after a failing period | Not found |
| Outbound HTTP | SSRF-safe client with DNS filtering and proxies | None |
| Webhook signing | Standard Webhooks, HMAC and Ed25519, key rotation | None (outbox leaves the broker to the service) |
| Secrets at rest | XChaCha20-Poly1305 main key | Outbox encryption, signed and encrypted sessions |
| Migrations | sqlx plus background migrations under advisory lock | `db migrate` per database crate; no background kind |
| Cache | none/memory/Redis, typed keys, cluster and sentinel | Redis or memory, mandatory expiry, fail-open loader; single Redis |
| Observability | OTLP traces, metrics, logs; Sentry | OTLP traces and metrics, Prometheus, redaction |
| Health | Ping plus a check that writes to queue and cache on every call | Live, ready, startup; cached, time-limited, never quotes errors |
| OpenAPI | Code-first `aide`, doc comment required by macro | `utoipa` or hand-written, drift checks, pinned Swagger UI |
| Rate limits | None | `axa-request-rate-limit` |
| Pagination | Unsigned id iterators, forward and back | Signed cursors, forward with `Link` |
| Tests | Real server per test, `TestReceiver` for callbacks | `axa-test` request helpers |

## What Axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| SSRF-safe outbound HTTP client | Any service that fetches a user-given URL (webhooks, avatars, imports) needs it, and it is easy to get wrong | M | High |
| Standard Webhooks signing and verifying | Generic spec; many services send or receive webhooks, and key rotation needs several signatures | S | High |
| Field path on JSON body errors | A caller learns which field was wrong without the value being quoted | S | High |
| Jitter and job-chosen retry delay in the job queues | Stops retries arriving in step; lets a job honour a peer's `Retry-After` | S | Medium |
| Background (online) migrations | `CREATE INDEX CONCURRENTLY` cannot run in a migration transaction, and must not block start-up | M | Medium |
| `PATCH` tri-state field type | Tells "not sent" from "set to null" without hand-written serde | S | Medium |
| Test receiver for outbound calls | Tests a service's callbacks and outbox relays end to end | S | Low |
| Redis cluster and sentinel in `axa-redis-cache` | Some operators run only these | M | Low |
| `#[track_caller]` location trail on errors | Cheap error paths in logs | S | Low |
| Sentry export | Common, but OTLP already reaches most error trackers | S | Low |

**SSRF-safe outbound HTTP client.** A new crate, `axa-outbound-http`, would wrap
`reqwest` with a custom `dns::Resolve` that drops blocked addresses after resolution, as
Svix's `NonLocalDnsResolver` does, and also checks a literal IP in the URL. Blocked by
default: the full list Svix uses, built on `ipnet`. The API would be a builder,
`OutboundClient::builder().allow_net(..).allow_host(..).timeout(..).https_only(true)
.build()`, with redirects off by default, or each redirect target checked again.
Responses would be read up to a size cap. Errors would be a typed `Blocked` versus
transport failure, so a caller can answer `422` for a bad URL. Production would refuse
`allow_net` of loopback, following Axa's rule of refusing unsafe settings.

**Standard Webhooks signing and verifying.** A small crate, `axa-webhook-signature`,
with `sign(id, timestamp, body, &[Key]) -> HeaderValue` and a verifier that checks the
timestamp against a tolerance, accepts any listed signature, and compares in constant
time. Keys are `Hmac` or `Ed25519` with the `whsec_`/`whpk_` prefixes. A verifier
extractor for Axum would read `webhook-id`, `webhook-timestamp`, and `webhook-signature`
and the raw body, and reject with an `axa::problem`. The outbox relay could use `sign`
when the service's broker is HTTP. It depends on `hmac`, `sha2`, `ed25519-dalek`, and
`base64`.

**Field path on JSON body errors.** In `axa::extract::Json`, deserialise through
`serde_path_to_error` instead of `axum::Json`, and put the path (such as
`/items/0/name`, as a JSON Pointer) in the problem's `errors`, with a fixed message per
serde error kind. The value stays out, as now. This fits Axa's existing
`ModelValidationErrors` shape and keeps the "quotes nothing" rule.

**Jitter and job-chosen retry delay.** In `axa-postgres-job-queue` and the MySQL and
SQLite queues, add `QueueOptions::retry_jitter` (a fraction, default perhaps 0.2) applied
to `backoff`, and let a job return an error that says "retry no sooner than this", which
the queue clamps to a range as Svix does. Both are small changes to the retry update
query.

**Background migrations.** In each database crate, beside `migrate`, a
`BackgroundMigration { id, apply, cleanup, revert }` list run by a worker or by `db
migrate --background`, under `pg_try_advisory_lock`, with a status table like Svix's.
Postgres first, since `CONCURRENTLY` is a Postgres feature. It would show in
`/_health/startup` only if marked as needed to start.

**PATCH tri-state.** A `axa::patch::Field<T>` enum (`Absent`, `Null`, `Value(T)`) with
`Deserialize` that maps a missing field to `Absent` through `#[serde(default)]`, and
`validator` and `utoipa` support. It is a few dozen lines and saves every service from
writing `Option<Option<T>>` by hand.

## What Axa should not copy

- **Message, endpoint, and attempt models, and operational webhooks.** These are the
  product. Axa's outbox already leaves the broker to the service.
- **Open CORS by default.** Svix allows any origin. Axa keeps CORS off until origins are
  set.
- **Its idempotency design.** Re-running after five seconds and not fingerprinting the
  body can run a write twice or replay the wrong answer. Axa's Postgres design is safer.
- **Its health check.** It writes to the queue and cache on every call, with no caching
  or time limit, so a public route can drive load. Axa's readiness already avoids this.
- **Redis and RabbitMQ queues through `omniqueue`.** Axa's queues are in the database so
  that enqueueing joins the caller's transaction and acts are audited. A broker queue
  would lose both.
- **Unsigned id iterators.** Axa's cursors are signed, so a caller cannot forge one.
- **Warnings for conflicting settings.** Svix logs and carries on. Axa should keep
  refusing unsafe settings in production.
- **`--wait-for` TCP polling.** Kubernetes startup probes already do this, and Axa has
  `/_health/startup`.
- **A `/development/echo` route in debug builds.** It logs every header, which can
  include tokens.
- **The JWT-specific "you sent the secret" message.** It is useful for HS256, but Axa
  uses PASETO and Keycloak, where it does not apply.
