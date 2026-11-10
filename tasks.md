# Tasks

The work in [`plan.md`](plan.md), in its three stages. Each task follows the spec's rule: one
change carries the spec edit, the code edit, and the test. **Effort** is S, M, or L. **New
dependency** names a crate to raise first (P-6). **Sources** are the studies in
[`spec/compare/`](spec/compare/index.md) that point to it.

## Stage 1: close the holes, with no new dependency

Done in Axa 0.11.0 (#23). T-7 needed no `no_store` helper of its own: neither auth crate
issues a token, and a response that sets a cookie is marked `no-store` automatically. T-4
made `http-body-util` a direct dependency, which every build already had, and changed
`middleware::body_limit` to wrap a route.

| Id | Task | Where | Effort | Breaking | Sources |
|---|---|---|---|---|---|
| T-1 | Send `WWW-Authenticate: Bearer` on every `401`, with `error="invalid_token"` where RFC 6750 says, and no more detail than the `401` already gives | `axa-keycloak-auth`, `axa-paseto-auth` | S | No | actix-web |
| T-2 | `/_health/ready` answers `503`, code `shutting_down`, from the moment shutdown begins, so a draining pod leaves the Service before connections close. Liveness stays `200` | `src/health.rs`, `src/server.rs`, `spec/health` | S | Yes | tuwunel |
| T-3 | A shutdown signal a handler can wait on, such as `axa::server::Shutdown`, so a WebSocket or server-sent events stream ends itself rather than at `SHUTDOWN_TIMEOUT_SECONDS` | `src/server.rs` | S | No | actix-web, Rocket, tuwunel |
| T-4 | Bind the body limit on a streamed `Body` too, with `tower_http::limit::RequestBodyLimitLayer` beside `DefaultBodyLimit`, and keep `middleware::body_limit` raising both on one route | `src/middleware.rs` | S | No | Sentry Relay |
| T-5 | Answer `503` with `Retry-After` and code `unavailable` for a pool timeout, a closed connection, and a read-only database (SQLSTATE `25006`), from all four database crates | `src/error.rs`, database crates | S | Yes | crates.io, Rocket |
| T-6 | Add `permissions-policy` (a deny-list default, overridden by `HTTP_PERMISSIONS_POLICY`), `cross-origin-opener-policy: same-origin`, and `cross-origin-resource-policy: same-origin` to the security headers, each left alone when a handler sets its own | `src/middleware.rs` | S | Yes | Kanidm, Rocket |
| T-7 | `cache-control: no-store` on a response that sets a cookie, unless a handler set its own, and a `middleware::no_store` for credential routes. The auth and session crates use it on what they issue | `src/middleware.rs`, auth and session crates | S | Yes | Kanidm |
| T-8 | Compression skips a response that sets a cookie or is `no-store`, against BREACH | `src/middleware.rs` | S | No | Kanidm |
| T-9 | Mark `authorization`, `cookie`, `set-cookie`, and `proxy-authorization` sensitive in the standard stack, with `SetSensitiveHeadersLayer` | `src/middleware.rs` | S | No | Poem |
| T-10 | Request metrics in `axa-prometheus-metrics`: a layer recording a duration histogram and count by method, status class, and route template, with one `unmatched` label for no route | `axa-prometheus-metrics` | S | No | Atuin, Poem, Sentry Relay |

## Stage 2: capabilities with a clear safety case

| Id | Task | Where | Effort | New dependency | Sources |
|---|---|---|---|---|---|
| T-11 | An outbound HTTP client that refuses private, loopback, link-local, CGNAT, and metadata addresses after DNS lookup and on each redirect, connects only to the addresses it checked, and takes an allow-list. Its own threat model and spec section | New crate `axa-http-client` | M | `reqwest` | Svix, Windmill, tuwunel |
| T-12 | Webhook signing and verifying to the Standard Webhooks scheme: HMAC-SHA256 and Ed25519, several signatures while keys rotate, a timestamp window, constant-time comparison, and an extractor for the verifying side | New crate `axa-webhooks` | S | `hmac`, `sha2`, optionally `ed25519-dalek` | Svix, Windmill, Loco |
| T-13 | A cross-site write guard: refuse unsafe methods and WebSocket upgrades when `Sec-Fetch-Site` is `cross-site`, fall back to comparing `Origin` with the host, let requests with neither through, and allow a per-route opt-out | `src/middleware.rs` | S | None | Topcoat, Windmill, actix-web |
| T-14 | Bounded multipart uploads over `axum::extract::Multipart`: limits on total size, memory, parts, and each field, files spooled to a temporary file, a cleaned file name, and `413` or `415` problems | New module `axa::upload` | M | None, or `tempfile` | actix-web, Poem, Rocket |
| T-15 | A safe `Content-Disposition` for downloads: `inline` only for a short list of media types, `attachment` otherwise, and a cleaned file name | `axa::format` | S | None | tuwunel |
| T-16 | A database per test in `axa-test`: a schema per test, cloned from a template migrated once, dropped by a guard, for `axa-postgres-sqlx` first | `axa-test`, `axa-postgres-sqlx` | M | None | crates.io, Loco, Windmill |
| T-17 | Per-connection settings in the database crates: `statement_timeout`, `application_name`, and an optional read-only mode, from `config::Database` | Database crates, `src/config.rs` | S | None | crates.io |
| T-18 | Recover from the stale prepared statements a migration leaves: spot SQLSTATE `0A000` and retire the pooled connection | `axa-postgres-sqlx` | S | None | Windmill, Atuin |
| T-19 | Readiness that waits for migrations: with `pending` migrations, `/_health/ready` and `/_health/startup` fail and workers wait | `src/health.rs`, database crates | S | None | Windmill |
| T-20 | Request body decompression, with the body limit counted on decoded bytes and a test that a small body expanding past it gets `413` | `src/middleware.rs` | S | None (`tower-http` feature) | Sentry Relay |
| T-21 | A public settings reader over `Vars`, so a service's own variables get production refusals and redaction, and each secret may come from a `_FILE` path, refused if both are set | `src/config.rs` | M | None | Rocket, Atuin, Windmill |
| T-22 | Field paths in JSON body errors, as a JSON Pointer in the problem, quoting nothing sent | `src/extract.rs` | S | `serde_path_to_error` | Svix |

## Stage 3: wait for two consumers (P-2)

Each needs two services that ask, or one with a safety case, before it starts.

| Id | Task | Where | Effort | New dependency | Sources |
|---|---|---|---|---|---|
| T-23 | A mail crate with SMTP, file, and memory transports, production refusing insecure SMTP, and the memory transport reachable from `axa-test`. It does not queue mail itself | New crate `axa-mail` | M | `lettre` | Loco, Topcoat, crates.io |
| T-24 | Object storage for local disk, memory, and S3, refusing keys with `..`, with presigned URLs | New crate `axa-storage` | M | `object_store` | Loco, crates.io |
| T-25 | Rate limits shared across replicas: a token bucket in Postgres in one upsert, and a Redis store, the service choosing whether an outage lets requests through | `axa-request-rate-limit` | M | None, or `redis` | crates.io, actix-web |
| T-26 | Wake the Postgres queue with `LISTEN` and `NOTIFY`, polling kept as the fallback | `axa-postgres-job-queue` | M | None | crates.io |
| T-27 | Deduplicated enqueue by a key the caller names | Job queue crates | S | None | crates.io |
| T-28 | Retry jitter, and a job-chosen delay such as a receiver's `Retry-After`, within limits | Job queue crates | S | None | Svix |
| T-29 | Cron and calendar schedules beside `Schedule::every`, keeping each due time queued once | Job queue crates | M | A cron parser | Loco |
| T-30 | Background migrations for SQL that cannot run in a transaction, such as `CREATE INDEX CONCURRENTLY`, under an advisory lock, each recorded | `axa-postgres-sqlx` | M | None | Svix |
| T-31 | An optional internal listener, `INTERNAL_PORT`, for the health routes and metrics, with the spec's health requirements updated to match | `src/server.rs`, `spec/health` | M | None | Atuin, Sentry Relay |
| T-32 | A static token or Basic credential guard for operator routes, compared in constant time | Auth crates | S | None | actix-web |
| T-33 | Opaque API tokens: a prefix, stored as SHA-256, with scopes, expiry, and last use | New module or crate | M | None | crates.io |
| T-34 | An absolute session deadline counted from sign-in | `axa-web-session` | S | None | actix-web |
| T-35 | A tri-state `Patch<T>` that tells a missing field from one set to null | `axa::extract` | S | None | Svix, Poem |
| T-36 | `extract::Form` that rejects with a problem, and a form context of values and field errors for a Tera page | `axa::extract`, `axa-tera-templates` | M | None | Rocket |
| T-37 | A `health` command that probes the local listener, for an image with no `curl` | `src/cli.rs` | S | None | tuwunel |
| T-38 | Tokio runtime and task metrics in the Prometheus registry | `axa-prometheus-metrics` | S | `tokio-metrics` | Poem |
| T-39 | PROXY protocol v1 and v2 from trusted ranges, for layer-4 load balancers | `src/server.rs` | M | `ppp` or similar | Kanidm |
| T-40 | An OpenAPI conformance check in `axa-test`, testing real response bodies against the documented schema | `axa-test` | M | `jsonschema`, dev only | Poem |
| T-41 | Captured logs and mail in `axa-test` | `axa-test` | S | None | Loco, tuwunel |
| T-42 | A trailing-slash policy: a `308` to the mounted form, keeping the query, without getting around the auth crates' own checks | `src/routes.rs` | S | None | Topcoat, Poem |

Lower-rated ideas, each on its project's page: a read-replica pool, row-level security context
per transaction, an SSE helper, an `Accept-Language` helper, CSP script hashes, in-process TLS,
Unix sockets, a version header, a capped blocking pool for password hashing, operator traffic
blocks, and continuing an incoming `traceparent`.
