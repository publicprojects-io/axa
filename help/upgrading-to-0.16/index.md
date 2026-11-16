# Upgrading to Axa 0.16

This covers a service on 0.15. Most services upgrade by changing the version and fixing
what the compiler names below. A service on the Postgres queue's `connect_existing` runs
the queue's setup again, and a service with sessions reads the new sign-in lifetime.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.7 is the one for `axa` 0.16 | Set `axa-test = "0.7"` in `[dev-dependencies]` |
| `config::Server` has `internal_port` and `proxy_protocol_from` | A service that builds a `Server` field by field adds `internal_port: None` and `proxy_protocol_from: Vec::new()`. One that reads it from `Config::from_env` changes nothing |
| `cli::Command` has a `Health` variant | A `match` on it adds an arm that calls `axa::cli::health` before connecting to anything |
| `axa_web_session::layer(store)` is `layer(store, &options)`, answering a `Stack` of `SignInDeadline` and `SessionLayer` | Pass the `SessionOptions` the service already builds, as `&options`. A service that names the layer's type names the `Stack` |
| `SETUP_STATEMENTS` in `axa-postgres-job-queue` has two more statements | Nothing, unless you match on the array's length |

## What changes with no compile error

**A session ends a week after sign-in**, however busy it has been, as well as after its
idle lifetime. Set `SessionOptions::sign_in_lifetime` for another span. A session signed
in before the upgrade has no time of sign-in, so it runs on to its idle lifetime as
before.

**A trailing slash on a mounted path is a `308`** to the path without it, keeping the
query, method, and body: `/api/folders/` goes to `/api/folders`. A client that sent the
slash and got a `404` now follows the redirect. A path with nothing mounted behind it is
still the `404`.

**A retry waits a random time in the upper half of its backoff**, in all three job
queues, rather than the backoff exactly, so replicas retrying together spread out. A
handler that fails with `RetryAfter`, or a worker with `axa::Error::RetryAfter`, chooses
its own delay, held between a second and an hour.

**The Postgres queue listens for new jobs.** Setup adds the `axa_queue_notify` trigger,
which announces each queued job on the `axa_queue` channel as its transaction commits,
and `Queue::run` holds one connection of its pool to listen, so an idle worker starts a
job at once. `Queue::connect` adds the trigger at boot. A service on
`Queue::connect_existing` runs the two new `SETUP_STATEMENTS` in a migration; until it
does, the queue polls as before. Size the pool for the one connection more.

**`settings()` lists `MAX_CONNECTIONS` once**, where it listed it twice, and lists the new
`INTERNAL_PORT` and `PROXY_PROTOCOL_FROM`.

## What is new

- `axa::extract::Patch<T>`, for a `PATCH` field that tells one left out from one set to
  `null`.
- `axa::extract::Form` and `FormContext`: a form rejected with a problem, and its values
  and field errors for a Tera page.
- `axa::operator::protect`, a bearer token or Basic credential guard for operator
  routes, compared in constant time.
- `service health [--ready]`, which asks the local server for its health, for an image
  with no `curl`.
- `INTERNAL_PORT`, a second listener for the health routes and `/metrics.prom`, which the
  main port then answers `404`.
- `PROXY_PROTOCOL_FROM`, the ranges whose connections start with a PROXY protocol header
  naming the client, for a layer-4 load balancer.
- The `api-tokens` feature, `axa::api_tokens`, from the new `axa-api-tokens`: opaque
  tokens kept as a SHA-256 hash, with scopes, expiry, and revocation.
- The `ratelimit-postgres` and `ratelimit-redis` features: a request limit shared across
  replicas, `axa_request_rate_limit::shared::SharedLimiter`.
- The `prometheus-tokio` feature: Tokio runtime and task metrics,
  `axa::prometheus::runtime::TokioMetrics`.
- Background migrations in `axa-postgres-sqlx`, `Background` and `run_background`, for
  statements such as `CREATE INDEX CONCURRENTLY`.
- In `axa-test`: `logs::capture`, the `mail` feature's `mail::outbox`, and the `openapi`
  feature's `openapi::Conformance`, which checks a real response against the OpenAPI 3.1
  document.

The published `axa` still leaves out the `sqlite-*` features, as since 0.13.
