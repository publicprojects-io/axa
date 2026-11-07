---
name: axa-skill
description: Build, configure, secure, test, and deploy Rust web services with axa, the small shared library for services built directly on Axum. Use this whenever a project depends on the `axa` crate or any `axa-*` crate, or the user asks about `AppContext`, `AppRoutes`, `Routes`, `axa::Error`, `Config::from_env`, `cli::parse`, `BackgroundWorker`, the axa job queue, `axa-sqlx`, `axa-seaorm`, `axa-keycloak`, `axa-paseto`, or moving a Loco service onto axa, even if they only say "my Axum service" and the manifest shows axa. Not for changing axa's own source; use axa-maintainer-skill for that.
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
skill describes 0.5, and says so where an earlier version differs.

## Where the truth lives

Read these rather than guessing at an API:

| Need | Read |
|---|---|
| The API of a version | [docs.rs/axa](https://docs.rs/axa) |
| Everything axa provides, with examples | [README](https://github.com/publicprojects-io/axa/blob/main/README.md) |
| Exact behaviour, and the test proving it | [Specification](https://github.com/publicprojects-io/axa/blob/main/spec/index.md) |
| What changed between versions | [Changelog](https://github.com/publicprojects-io/axa/blob/main/CHANGELOG.md) |
| Upgrading to 0.3 and 0.4 | [Upgrade guide](https://github.com/publicprojects-io/axa/blob/main/help/upgrading-to-0.3/index.md) |
| Leaving Loco | [From Loco into axa](https://github.com/publicprojects-io/axa/blob/main/help/from-loco-into-axa/index.md) |
| Leaving Loco but keeping SeaORM | [From Loco SeaORM into axa-seaorm](https://github.com/publicprojects-io/axa/blob/main/help/from-loco-seaorm-into-axa-seaorm/index.md) |

## Install

```toml
[dependencies]
axa = { version = "0.5", features = ["sqlx", "queue"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

[dev-dependencies]
axa = { version = "0.5", features = ["testing"] }
```

Turn on only the features a service uses, since each one adds to what an auditor must
read. It needs Rust 1.97 or newer, on edition 2024.

| Need | Feature |
|---|---|
| Postgres through sqlx, the preferred way. `AppContext::db` becomes a `PgPool` | `sqlx` |
| Postgres job queue | `queue` |
| Keep SeaORM | `sea-orm` |
| Diesel, with no `libpq` | `diesel` |
| Request-level tests | `testing` |
| Keycloak bearer tokens and ABAC | `keycloak` |
| PASETO bearer tokens and ABAC | `paseto` |
| Browser sessions | `session` |
| HTML templates | `tera` |
| Redis cache with mandatory expiry | `redis` |
| Offset pages and `LIKE` escaping | `pagination` |
| OpenAPI documents and Swagger UI | `openapi` |
| Redaction, request tracking, async diagnostics | `tracing` |
| OTLP traces and request metrics | `opentelemetry` |
| Prometheus exposition | `prometheus` |
| RFC 9457 problem details | Always included, as `axa::problem` |
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
- Axum's own refusals are not problems: an unmatched path's `404`, a wrong method's `405`,
  and the rejection of an extractor a handler takes directly. Take
  `Result<Json<T>, JsonRejection>` and return the rejection with `?` to get a problem.
- A `"/"` route is served at the prefix with no trailing slash, and two methods on one
  path merge.
- Give a slow route its own limit where it is declared, with `Routes::timeout` or
  `Routes::add_with_timeout`, rather than raising `REQUEST_TIMEOUT_SECONDS` for all.
- Raise the body limit on one route with `middleware::body_limit`, not on all.

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
  is set. `--environment` cannot lift this. `Cli::parse` still does not refuse them, so
  a service that keeps it must call the guard itself.

`Migrator::fresh` does not pass through any migration's `down`, so a guard written in
a `down` does not protect `db reset`.

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
