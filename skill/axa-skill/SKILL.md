---
name: axa-skill
description: Build, configure, secure, test, and deploy Rust web services with axa, the small shared library for services built directly on Axum. Use this whenever a project depends on the `axa` crate or any `axa-*` crate, or the user asks about `AppContext`, `AppRoutes`, `Routes`, `axa::Error`, `Config::from_env`, `cli::parse`, `BackgroundWorker`, the axa job queue, `axa-sqlx`, `axa-seaorm`, `axa-keycloak`, `axa-paseto`, `axa-idempotency`, `Idempotency-Key`, the outbox, `axa-worker`, scheduled jobs, or moving a Loco service onto axa, even if they only say "my Axum service" and the manifest shows axa. Not for changing axa's own source; use axa-maintainer-skill for that.
---

# Using axa in your Axum service

[axa](https://github.com/publicprojects-io/axa) holds the plumbing every Axum service
needs: route tables, a standard middleware stack, health checks, error responses,
configuration, logging, graceful shutdown, a command line, background jobs, and a
Postgres job queue.

It is a library, not a framework. `main.rs` stays ordinary Rust that calls ordinary
functions, `AppContext` is a plain struct passed as Axum state, and `AppRoutes` builds
an ordinary `axum::Router`. Write code that keeps that true: an auditor should be able
to read the whole path from `main.rs` down. Reach for plain Axum whenever axa has no
opinion, because everything axa returns is plain Axum.

Check the version in the project's `Cargo.toml` before relying on a detail here. This
skill describes 0.7, and says so where an earlier version differs.

## Where the truth lives

Read these rather than guessing at an API:

| Need | Read |
|---|---|
| The API of a version | [docs.rs/axa](https://docs.rs/axa) |
| Everything axa provides, with examples | [README](https://github.com/publicprojects-io/axa/blob/main/README.md) |
| Exact behaviour, and the test proving it | [Specification](https://github.com/publicprojects-io/axa/blob/main/spec/index.md) |
| What changed between versions | [Changelog](https://github.com/publicprojects-io/axa/blob/main/CHANGELOG.md) |
| Upgrading to 0.3 and later | [Upgrade guide](https://github.com/publicprojects-io/axa/blob/main/help/upgrading-to-0.3/index.md) |
| Leaving Loco | [From Loco into axa](https://github.com/publicprojects-io/axa/blob/main/help/from-loco-into-axa/index.md) |
| Leaving Loco but keeping SeaORM | [From Loco SeaORM into axa-seaorm](https://github.com/publicprojects-io/axa/blob/main/help/from-loco-seaorm-into-axa-seaorm/index.md) |
| Where the idempotency layer goes | [Guide](https://github.com/publicprojects-io/axa/blob/main/help/idempotency-keys/index.md) |

## Install

```toml
[dependencies]
axa = { version = "0.7", features = ["sqlx", "queue"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

[dev-dependencies]
axa = { version = "0.7", features = ["testing"] }
```

Turn on only the features a service uses, since each one adds to what an auditor must
read. It needs Rust 1.97 or newer, on edition 2024.

| Need | Feature |
|---|---|
| Postgres through sqlx, the preferred way. `AppContext::db` becomes a `PgPool` | `sqlx` |
| Postgres job queue | `queue` |
| Transactional outbox and relay on Postgres | `outbox` |
| The outbox, with payloads sealed at rest | `outbox-encryption` |
| Keep SeaORM | `sea-orm` |
| Diesel, with no `libpq` | `diesel` |
| Request-level tests | `testing` |
| Keycloak bearer tokens and ABAC | `keycloak` |
| PASETO bearer tokens and ABAC | `paseto` |
| Browser sessions | `session` |
| HTML templates | `tera` |
| Redis cache with mandatory expiry | `redis` |
| Signed cursor pages, no total, and `LIKE` escaping | `cursor-pagination` |
| Offset pages with a total (insecure, prefer cursors) | `offset-pagination`, or the old `pagination` |
| OpenAPI documents and Swagger UI | `openapi` |
| Redaction, request tracking, async diagnostics | `tracing` |
| OTLP traces and request metrics | `opentelemetry` |
| Prometheus exposition | `prometheus` |
| RFC 9457 problem details | Always included, as `axa::problem` |
| Request limits per client or per key, and failed-login lockout | `ratelimit` |
| `Idempotency-Key` on writes, over Postgres | `idempotency` |
| `Accepts-version` negotiation | `versioning` |

`features` lists one name per subcrate, and each subcrate is also usable alone.
`Environment` lives in `axa-environment`, which has no dependencies, so a migration
crate can read the environment the same way without depending on `axa`.

## A service

```rust
use axa::{
    Environment, Result,
    config::{Config, Defaults},
    logger,
    prelude::*,
    server,
};

async fn hello() -> Result<Response> {
    format::json(serde_json::json!({ "hello": "world" }))
}

async fn run() -> Result<()> {
    let environment = Environment::from_env();
    let defaults = Defaults {
        // An empty name means this service has no database.
        database: "",
        ..Defaults::default()
    };
    let config = Config::from_env(&environment, &defaults)?;
    logger::init("hello_service", &config.logger);

    let server_config = config.server.clone();
    let ctx = AppContext::new(environment, (), config);

    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
        .into_app(ctx)?;

    server::serve(router, &server_config).await
}
```

It serves `/api/hello`, `/_ping`, `/_health`, and `/_readiness` on port 1241 under the
standard middleware, and stops cleanly on `SIGTERM` or Ctrl-C.

With layers of your own, such as an auth guard, call `into_router`, add them, and then
call `middleware::apply`. Added after `into_app`, a layer sits outside the standard
stack: its refusals have no request id, security headers, or CORS headers, and it has no
panic or timeout protection.

With the `sqlx` feature, pass a pool where this passes `()`. Then `/_readiness` asks it
as `database`, so do not add it again as a named check:

```rust
let pool = axa::db::connect(&config.database).await?;
let ctx = AppContext::new(environment, pool, config);
```

Parse the command line with `axa::cli::parse`, not `Cli::parse`, so that `--help` and
`--version` describe the service:

```rust
let cli = axa::cli::parse(
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_VERSION"),
    env!("CARGO_PKG_DESCRIPTION"),
);
```

## Writing handlers

- Return `axa::Result<T>`. A failed database call then answers a generic `500`, and the
  real error goes only to the log. Do not build your own error text from a database
  error, which is how a hostname or a row reaches a client.
- Use `not_found()`, `unauthorized(..)`, and `bad_request(..)`. Only `BadRequest` and
  `CustomError` text reaches the caller, so treat that text as public.
- An error's body is an RFC 9457 problem, `application/problem+json`, and a success is
  bare JSON. Read the reason from `code`, the text from `detail`, which can be absent, and
  field errors from `errors`. `ErrorDetail::new("code", "detail")` sets the first two.
- The standard stack makes any error sent as plain text or with no `content-type` a
  problem, and drops the text. That covers Axum's own `404`, `405`, and extractor
  rejections, and also `(StatusCode::CONFLICT, "why")` from a handler, so return an
  `Error` to say why. Take `Result<Json<T>, JsonRejection>` and return the rejection with
  `?` to tell the caller what was wrong with a body.
- A `"/"` route is served at the prefix with no trailing slash, and two methods on one
  path merge.
- Give a slow route its own limit where it is declared, with `Routes::timeout` or
  `Routes::add_with_timeout`, rather than raising `REQUEST_TIMEOUT_SECONDS` for all.
- Raise the body limit on one route with `middleware::body_limit`, not on all.

## Conditional requests

- The standard stack answers `304` for you. Set an `ETag` on the answer and the handler
  need not read `If-None-Match`: `Ok((tag, format::json(body)?).into_response())`.
- A body with a timestamp in it, such as `as_of`, needs a tag that leaves it out:
  `ETag::weak_of(&payload)?`, taken before the timestamp goes in.
- `conditional::etag` tags a route's answers from the body. It is opt-in. Add it before
  `apply`, outside anything that shapes the body such as paging, and outside what sets
  `Cache-Control`, so it can see `no-store` and skip the hash.
- A `304` comes after the handler has run, so it saves bytes and not the query.
- `If-Match` compares strongly, so use `ETag::strong` or `ETag::strong_of`. A weak tag
  never passes, and every write answers `412`. Call `conditional::if_match` after
  authorising the caller and loading the record, and put the check and the write in one
  transaction or a `WHERE version = $1`.

## Paging collections

- Page with `cursor-pagination`. A list takes `?limit=` and an opaque, signed `?cursor=`,
  answers a bare JSON array, and sends `Link: <...>; rel="next"` while more rows follow.
  It sends **no total**, because a total counts rows the caller's record policy hides.
- Build one `CursorKey::new(secret)?` at boot from a secret of 32 bytes or more, kept
  like a session key. Give each endpoint its `CursorLimits { default_limit, max_limit }`.
- In the handler, `let page = CursorPage::new(&key, limits, &uri)?;`. A bad limit or
  cursor answers `422` through `?`, and a limit above the maximum is refused, never
  clamped. Bind `let (limit, offset) = page.bounds();` as `LIMIT $1 OFFSET $2`, then
  return `page.respond(rows)`.
- Order every list by something ending in a unique column, such as
  `ORDER BY created_at, id`.
- Pass user text for `LIKE` through `escape_like` and bind it with `ESCAPE '\'`.
- `offset-pagination` sends `X-Total-Count` and takes `?offset=`. It is insecure, and
  new code does not use it.

## Configuration

There are no config files. `Defaults` states a service's defaults in code, and the
environment overrides them. `check` prints the effective settings with secrets
redacted, so run it first when a deployment misbehaves. Reach for `Config::from_lookup`
in tests, which takes the lookup as a function and leaves the process environment alone.

Production fails closed, and these are the usual first-deploy surprises:

- `DATABASE_URL` is required when a service has a database. It must set `sslmode` to
  `require`, `verify-ca`, or `verify-full`, unless it stays on loopback or a socket.
  `DB_ALLOW_INSECURE=true` is the way out for a network that encrypts its own traffic.
- A set but invalid value, such as `HTTP_HSTS=tru`, stops the boot and names the variable.
- CORS sends nothing until `CORS_ALLOWED_ORIGINS` lists origins, and `*` is refused.
- Migrations do not run at boot unless `DB_AUTO_MIGRATE` says so.
- Any environment name other than `development` or `test` gets production's behaviour.
- `X-Forwarded-For` is believed only from proxies listed in `TRUSTED_PROXIES`. A handler
  takes `axa::client::ClientAddr` for the real address, which stays out of the request
  span because it is personal data.
- `MAX_CONNECTIONS` limits open connections, and defaults to `server::MAX_CONNECTIONS`.

## Destructive commands

`db down`, `db reset`, and `db truncate` destroy data.

- In 0.4.0, `Cli` only parses them, so a service must call
  `Environment::may_destroy_data` itself before carrying one out.
- From 0.4.1, `cli::parse` refuses them before the service runs
  anything, unless the process is in development or test and `AXA_DESTROY_DATA=true`
  is set. `--environment` cannot lift this. `Cli::parse` refuses them too.
- Mark a task that loads fixtures with `Task::development_only`, so it cannot run in
  production.

`Migrator::fresh` does not pass through any migration's `down`, so a guard written in
a `down` does not protect `db reset`.

## Errors a client sees

- Use `axa::extract::{Json, Path, Query}` in handlers where a caller should be told what
  was wrong. Axum's, which the prelude still exports, reject with a problem with no
  `detail`.
- A violated unique or foreign key constraint answers `409`, so a handler need not map
  it unless it means something else.
- `PageParams::resolve` works with `?`. `Page::bounds` gives `LIMIT` and `OFFSET`, and
  `Page::respond` answers the rows with the count headers.

## Connections

The server speaks HTTP/1 alone, and expects TLS to end in front of it. Keep
`KEEP_ALIVE_TIMEOUT_SECONDS`, 75 by default, above any load balancer's idle timeout, and
`MAX_CONNECTIONS`, 10,000 by default, under the process's open-file limit. A client that
stops reading a response is dropped after `SEND_STALL_TIMEOUT_SECONDS`, 60 by default.

## Jobs

- `ReportWorker::perform_later(&ctx, args)` follows `WORKER_MODE`.
- **Nothing runs jobs by itself.** A service needs `start --all` or a separate
  `start --worker`. Call `queue::warn_if_unserved` at boot so a forgotten worker shows up.
- A job gets one attempt by default, because a retry repeats its work. Raise
  `max_attempts` only for jobs written to be repeated.
- Use `queue::enqueue_on` to queue on the caller's sqlx transaction, so the job and its
  record commit together. `Queue::enqueue` uses its own connection, which can leave a
  job for a record that rolled back.
- Queue arguments are stored in plain text, so never put a secret in them.
- `Queue::connect` runs DDL at boot. Where a service's role may not, run
  `queue::SETUP_STATEMENTS` in a migration and call `Queue::connect_existing`.
- `QueueOptions::workers` is how many jobs a process runs at once. `queue.limit::<_, W>(n)`
  holds one job type below that, per process, not across replicas.
- A job has **no time limit unless its worker sets `timeout`**. Delay with `perform_in`,
  or build a job with `queue::job::<W, _>(&args)?` and `run_at`, `priority`, `timeout`.
- Keep the id `perform_later` returns to call `queue.job`, `cancel`, or `retry`. Expose
  them only behind the service's own authorisation, and never send a job's error text
  to a client.
- A job in **`needs_review`** lost its claim and was stopped, so it may have run in part
  or twice. Nothing retries it: check what it did, then call `queue.retry`.
- **Schedules** are `queue.every::<_, W>("name", interval, &args)?` in code, never config.
  Each due time is queued once across replicas, missed times come to one job, and a time
  is skipped while the last job runs. Schedule the `axa-idempotency` sweep this way. A
  schedule removed from code is archived, and returns if put back.

## Idempotency keys

With the `idempotency` feature, a retried write gets its first answer back instead of
running twice, once a service mounts the layer. It is unreleased: check that the
service's version has it.

```rust
let keys = Idempotency::new(pool, |request| {
    request.extensions().get::<Allowed>().map(|allowed| allowed.claims.sub.clone())
});
keys.check().await?;

let router = routes
    .into_router(ctx.clone())
    .layer(from_fn_with_state(keys, idempotent))
    .layer(from_fn_with_state(guard, paseto::middleware));
let app = axa::middleware::apply(router, &ctx.config.http)?;
```

Get these right, because each fails quietly or later:

- **The order is the point.** The layer goes inside `apply`, so that it stores the plain
  body, and inside the guards, so that a replay reaches only a caller they let through.
  Do not use `into_app`, which applies the stack before your layers.
- **The caller comes from the service.** The closure reads what authentication left on
  the request. `None` refuses the request with a `401`. Returning one text for all
  anonymous callers makes them share answers.
- **Run `SETUP_STATEMENTS` in a migration**, and call `check` at boot.
- **Call `sweep` on a schedule. This is required.** Nothing runs by itself, and an expired
  answer stays on disk until then. It can hold personal data, so add `axa_idempotency` to
  your record of personal data and retention list.
- **`forget_caller` erases by who asked**, not by whose data an answer holds. An answer to
  staff about a patient stays until it expires, so a privacy notice must allow for
  erasure taking up to `ttl`.
- **`required` refuses a write with no key**, with a `400`. A key is optional otherwise.
- **A `4xx` or `5xx` frees the key**, and a retry runs again. Any other answer that
  cannot be kept (a stream, an answer over 1 MiB, one a handler compressed) gets the
  retry a `409`, because the write ran. Only a `2xx` or `3xx` is replayed.
- **A crash after the handler commits repeats it**, as the answer is stored separately. A
  handler that must never repeat also needs a unique constraint. A `202` for a job
  replays even after the job fails, unless the service calls `forget`.

[Where the idempotency layer goes](https://github.com/publicprojects-io/axa/blob/main/help/idempotency-keys/index.md)
has the whole of this.

## Outbox

Use the outbox, not a broker call after the commit, whenever an event must not be lost.
The [guide](../../help/transactional-outbox/index.md) has the whole of it. `axa-outbox`
0.2 keeps a record: events and attempts are only ever inserted, and triggers refuse
every change.

- `outbox::write(&mut transaction, &event)` takes a transaction, never a pool, and
  `write_seaorm` takes SeaORM's. Change the record first: its row lock is what keeps one
  key's events in commit order. A payload over 64 KiB fails the write. For other limits,
  denied fields, or sealing, build one `outbox::Writer` at boot and call its `write`.
- **Nothing relays by itself.** Spawn `Outbox::run(decide, publish, record, stopped)` in
  `main.rs`, and stop it after `server::run` returns. The broker client stays in the
  service. With no rule or no audit, pass `decide::deliver_all` or `record::nothing`.
- `decide` runs in the claim's transaction and answers `Decision::Deliver`,
  `Decision::refuse("code")`, which is final, or `Decision::Hold`, which charges nothing.
  `record` runs in each outcome's transaction, so write the audit row there.
- Answer `Ok` only once the broker has acknowledged. A publisher that logs, or a send that
  only queues locally, records deliveries nobody received.
- Say which failure it was, with a short code, never the broker's text:
  `Failure::unavailable("broker_down")` charges nothing and waits, and
  `Failure::rejected("code")` charges this event, which ends as `failed` after
  `max_attempts`.
- Delivery is at least once, so a consumer must dedupe on `event_id`:
  `outbox::inbox::seen(&mut transaction, event_id)` in the consumer's transaction.
- Watch `Outbox::stats`: `failed` and `tampered` need a person, and `held` with
  `oldest_held` shows a forgotten hold. Put a failed event back with
  `outbox::release(&mut transaction, event_id, by, reason)`.
- Nothing is swept. Erase a payload with `outbox::erase(&mut transaction, event_id, by,
  reason)`, which keeps who and why. Prefer an id to a snapshot, and never put a secret
  in a payload.
- `Outbox::connect` runs DDL at boot. Where a service's role may not, run
  `outbox::SETUP_STATEMENTS` in a migration and call `Outbox::connect_existing`, which
  refuses to start without the triggers. `axa_outbox_owed` is derived working state:
  never edit it, and if it is ever wrong, run `outbox::REBUILD_OWED_STATEMENTS`.

## Authentication

`axa-keycloak` and `axa-paseto` both give a `Guard`, a `middleware`, and an `AuthUser`
extractor over ABAC policy. Every path is protected unless it is listed as public, and
enforcement is on unless a service turns it off. Refused callers learn nothing about
why: a `401` says `invalid token` and a `403` says `forbidden`, with the reason in the
log, so do not match on any other text. Each refusal writes one `auth_refused` line at
`warn`, with the status, action, and client address, and it never holds the token or
email. It replaces `paseto_forbidden` and `keycloak_forbidden`, so update any alert that
watches for those.

Both refuse a decision with an obligation the service has not declared, so a rule that
asks for `"audit"` is a `403` until the service declares it with `Guard::honouring`.
The `Allowed` extractor gives a handler the decision.

With `axa-keycloak`:

- Every token must carry a configured audience. Keycloak adds a client to `aud` only
  through an audience mapper, so a client without one gets a `401`.
- ID tokens, refresh tokens, and tokens dated in the future are refused.
- Roles grant nothing unless `role_map` names them, so a realm with no role map yields
  read-only callers under the default policy.
- Until discovery succeeds every token is refused, and the service still boots. Use
  `KeycloakVerifier::ready` for `/_readiness`.

With `axa-paseto` 0.3 and later, `parse_bool` returns `Option<bool>`. `None` is a word it does not
know, so refuse to start on it, because a typo read as off could turn enforcement off.

## Testing

With the `testing` feature, implement `TestApp` to say how a service boots for tests. It
is the one trait axa asks a service to implement, and it exists only for tests.

```rust
use axa::testing::prelude::*;

#[tokio::test]
async fn health_answers() {
    request::<App, _, _>(|server, _ctx| async move {
        assert_eq!(server.get("/_health").await.status_code(), 200);
    })
    .await;
}
```

Use `AppRoutes::mounted` for a route census: it lists every route as `(method, full
path)`, without booting anything.

## Observability

- `logger::init` writes logs. `logger::init_with` adds layers beside them, which is how
  `axa-opentelemetry` joins in.
- `axa-opentelemetry` exports nothing until `OTLP_ENDPOINT` is set, and refuses plain
  `http` to a remote collector unless `OTLP_ALLOW_INSECURE` is set. A log event inside
  a span travels with that span, so treat the collector as you treat the logs.
- Use `axa_tracing::Redacted` for a field that must not reach a log.

## What axa will not add

Route macros, a generic CRUD controller, a dependency-injection container, a plugin
system, a configuration language, and a database abstraction are out on purpose. Write
these in the service, in plain Rust, rather than looking for an axa feature. Domain
logic, handlers, models, and SQL stay in each service.
