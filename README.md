# axa

**axa** is a small shared library for web services built directly on
[Axum](https://docs.rs/axum), with [sqlx](https://docs.rs/sqlx) the preferred way to
reach a database.

It holds the plumbing every service needs and none of them should write twice:
route tables, a standard middleware stack, health checks, error responses,
configuration, logging, graceful shutdown, a command line, background jobs, and
a Postgres job queue.

The specification is [`spec/index.md`](spec/index.md). It is the single source
of truth, and every requirement in it names the test that proves it.

## Why it exists

A framework gives you a lot for free, and asks you to trust what it does on your
behalf. For a service handling identity, clinical, or government records, that
trade runs the wrong way: an auditor has to read the framework as well as your
code, and each of its dependencies joins your supply chain.

axa takes the opposite position:

- **You can read the whole path.** `main.rs` is ordinary Rust that calls ordinary
  functions. Nothing calls into your code from behind a trait.
- **One copy.** A defect in shared plumbing is fixed once, and an auditor reads
  it once.
- **Fewer dependencies.** Replacing Loco and SeaORM with sqlx took one measured
  service from 510 normal dependencies to about 348, before counting anything
  added to replace them.
- **Safe defaults that fail closed.** A production service with no database URL,
  or with a wildcard CORS origin, refuses to start.

## A library, not a framework

There are no lifecycle hooks, macros, or generated code. `AppContext` is a plain
struct, passed as ordinary Axum state, and `AppRoutes` builds an ordinary
`axum::Router`. Any piece can be read, replaced, or left out on its own.

Some things will deliberately never be added: route macros, a generic CRUD
controller, a dependency-injection container, a plugin system, a configuration
language, and a database abstraction. Each one puts indirection back between a
reader and the behaviour.

## Installing

Add it from crates.io:

```toml
[dependencies]
axa = "0.4"
```

| Feature | Adds |
|---|---|
| `queue` | The Postgres job queue |
| `testing` | Request-level test helpers, built on `axum-test` |
| `sea-orm` | A SeaORM-backed `AppContext`, plus [`axa-seaorm`](crates/axa-seaorm): connections, migration helpers, and model errors, with an easy path from Loco |
| `diesel` | [`axa-diesel`](crates/axa-diesel): Diesel pooling, migrations, and test resets, with no `libpq` |
| `tera` | [`axa-tera`](crates/axa-tera): Tera templates with safe escaping and generic errors |
| `tracing` | [`axa-tracing`](crates/axa-tracing): redaction, request tracking across spawned tasks and peers, and diagnostics for blocked or stalled async code |
| `redis` | [`axa-redis`](crates/axa-redis): a Redis cache with mandatory expiry |
| `openapi` | [`axa-openapi`](crates/axa-openapi): OpenAPI documents, a pinned Swagger UI, route listing, and drift checks |
| `opentelemetry` | [`axa-opentelemetry`](crates/axa-opentelemetry): OTLP export of traces and request metrics, off until an endpoint is set |
| `session` | [`axa-session`](crates/axa-session): web sessions over `axum_session`, signed and encrypted in production, with form tokens. Nothing is stored for a visitor until they sign in. No service uses this yet |
| `sqlx` | [`axa-sqlx`](crates/axa-sqlx): pooling, migrations, and test resets. `AppContext::db` becomes a `PgPool` |
| `pagination` | [`axa-pagination`](crates/axa-pagination): offset pages, count headers, and `LIKE` escaping |
| `keycloak` | [`axa-keycloak`](crates/axa-keycloak): Keycloak bearer authentication and ABAC enforcement, standing alone so it can be audited alone |
| `paseto` | [`axa-paseto`](crates/axa-paseto): PASETO bearer authentication and ABAC enforcement. No service uses this yet. Enforcement is on unless turned off |
| `problem` | [`axa-problem`](crates/axa-problem): RFC 9457 problem details. No service uses this yet |
| `prometheus` | [`axa-prometheus`](crates/axa-prometheus): Prometheus text exposition |
| `versioning` | [`axa-versioning`](crates/axa-versioning): `Accepts-version` negotiation |

`Environment` lives in [`axa-environment`](crates/axa-environment), which has no
dependencies and is always included, so a migration crate can read the environment
the same way without depending on `axa`.

## Quick start

This is a complete service with one route:

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

It listens on port 1241 and serves `/api/hello`, `/_ping`, `/_health`, and
`/_readiness` under the standard middleware. It stops cleanly on `SIGTERM` or
Ctrl-C, letting requests in flight finish.

The default port spells the name: a=1, x=24, a=1.

## What it provides

### Routes

`Routes` is a group sharing a prefix, and `AppRoutes` collects the groups:

```rust
use axa::prelude::*;

async fn list() -> &'static str { "[]" }
async fn create() -> &'static str { "{}" }
async fn show(Path(id): Path<u32>) -> String { id.to_string() }

fn folders() -> Routes {
    Routes::new()
        .prefix("/api/folders")
        .add("/", get(list))
        .add("/", post(create))
        .add("/{id}", get(show))
}
```

A `"/"` route is served at the prefix with no trailing slash, and two methods on
one path merge. The fields are public, so a test can check what is mounted
without booting anything:

```rust
# use axa::prelude::*;
# async fn list() -> &'static str { "[]" }
let group = Routes::new().prefix("/api").add("/items", get(list));
assert!(group.handlers.iter().any(|handler| handler.uri == "/items"));
```

`AppRoutes::mounted` lists every route as `(method, full path)`, so a route census
test can compare what is served with what is written down. A `HEAD` that comes with
`GET` is left out, and a handler for every method shows as `ANY`. Axum keeps a
router's methods private, so this reads its debug text, and a test fails if that
text changes.

```rust
# use axa::prelude::*;
# async fn list() -> &'static str { "[]" }
let routes = AppRoutes::empty()
    .add_route(Routes::new().prefix("/api").add("/items", get(list)));
assert_eq!(
    routes.mounted(),
    [("GET".to_string(), "/api/items".to_string())]
);
```

A route or group can state its own time limit where it is declared. It replaces
`REQUEST_TIMEOUT_SECONDS` there, longer or shorter, and other routes are still cut
off on time:

```rust
# use axa::prelude::*;
use std::time::Duration;

# async fn export() -> &'static str { "" }
# async fn report() -> &'static str { "" }
let reports = Routes::new()
    .prefix("/api/reports")
    .timeout(Duration::from_mins(2))
    .add("/daily", get(report))
    .add_with_timeout("/export", get(export), Duration::from_mins(10));
# let _ = reports;
```

### Middleware

`AppRoutes::into_app` applies this stack, or call `middleware::apply` on any
router. From the outside in:

| Layer | Behaviour |
|---|---|
| Request id | Keeps a caller's `x-request-id` after removing unsafe characters, or mints a UUID. Always returned |
| Tracing | Logs each finished request, and puts the request id, method, and route template on every log line written while serving it. The real path and query string are left out, because either can hold a token or personal data |
| Panic catching | Turns a handler panic into a `500` |
| Security headers | `x-content-type-options`, `x-frame-options`, `referrer-policy`, and optionally HSTS. A handler's own value wins |
| CORS | Off unless origins are configured |
| Timeout | Answers `408` after `REQUEST_TIMEOUT_SECONDS`. `Routes::timeout` and `Routes::add_with_timeout` set a route's own, and `Http::timeout_overrides` sets one by path suffix |
| Compression | Gzip, Brotli, and Zstd when enabled |
| Body limit | Answers `413` over `BODY_LIMIT_BYTES`. `middleware::body_limit` raises it on one route |

### Errors

Handlers return `Result<T>` using `axa::Error`, which maps to a status and a JSON
body:

| Error | Status | Body |
|---|---|---|
| `Error::NotFound` | `404` | Fixed text |
| `Error::Unauthorized(_)` | `401` | Fixed text; yours is only logged |
| `Error::BadRequest(_)` | `400` | Your text |
| `Error::CustomError(status, detail)` | Yours | Yours |
| Anything else | `500` | Fixed text; the real error is only logged |

So a failed database call cannot put a hostname or a query in a client's
response. The `not_found`, `unauthorized`, and `bad_request` functions build
these for you.

The body is `{"error": ..., "description": ...}` today, and is moving to RFC 9457
`application/problem+json`.

### Configuration

There are no config files. A service states its defaults in code, and the
environment overrides them, so what a deployment runs with is visible in one
place.

| Variable | Effect |
|---|---|
| `APP_ENV` | `development`, `test`, or `production`. `LOCO_ENV` is also read. Any other name gets production's stricter behaviour. With neither set, a debug build is development and a release build is production |
| `PORT`, `BINDING`, `HOST` | Listening port, address, and public URL |
| `DATABASE_URL` | Connection string. Required in production for a service with a database |
| `DB_MIN_CONNECTIONS`, `DB_MAX_CONNECTIONS` | Pool size |
| `DB_CONNECT_TIMEOUT`, `DB_IDLE_TIMEOUT` | Pool timeouts, in milliseconds. 5 seconds and 10 minutes by default, or a service's `Defaults` |
| `DB_LOGGING`, `DB_AUTO_MIGRATE` | Statement logging, and migrating at boot. A service sharing a database it does not own sets `Defaults::auto_migrate` to `false` |
| `LOG_ENABLE`, `LOG_LEVEL`, `LOG_FORMAT`, `LOG_FILTER` | Logging. `RUST_LOG` wins over all of them |
| `BODY_LIMIT_BYTES` | Largest accepted request body |
| `HTTP_COMPRESSION`, `HTTP_HSTS` | Response compression and HSTS |
| `SHUTDOWN_TIMEOUT_SECONDS` | How long, once told to stop, to wait for requests and jobs. 25 by default, and `0` waits for as long as they take |
| `HSTS_MAX_AGE_SECONDS` | How long browsers keep to HTTPS. One year by default |
| `HTTP_CSP` | The `content-security-policy`. Production sends `default-src 'none'; frame-ancestors 'none'` unless this is set, and sends none if it is empty |
| `REQUEST_TIMEOUT_SECONDS` | Per-request timeout. `0` turns it off |
| `CORS_ALLOWED_ORIGINS` | Comma-separated origins |
| `WORKER_MODE` | `BackgroundQueue`, `BackgroundAsync`, or `ForegroundBlocking` |

Defaults change with the environment:

| Setting | Development | Test | Production |
|---|---|---|---|
| Database | Local, derived from the service name | Local, derived from the service name | **Required** |
| Migrate at boot | Yes | Yes | No |
| Compression | Off | Off | On |
| Request timeout | Off | Off | 60 seconds |
| HSTS | Off | Off | On |
| CORS | Any origin | Any origin | None until listed; `*` is refused |
| Log format | Compact | Compact, off by default | JSON |
| Jobs | Queue | In the foreground | Queue |

A setting that is set but invalid, such as `HTTP_HSTS=tru`, is a mistake, and a
silent default would hide it. In production and any unknown environment it stops
the boot, and the error names the variable and its value. Elsewhere the default
applies and `logger::init` prints a warning. A blank value counts as unset, which
is what a compose file sends for an unset variable. Flags take `1`, `true`, `yes`,
`on`, `0`, `false`, `no`, or `off`.

In development and test, the local database is
`postgres://postgres:postgres@localhost:5432/<name>_<environment>`.

`Config::from_lookup` takes the lookup as a function, so configuration can be
tested without touching the process environment:

```rust
use axa::{Environment, config::{Config, Defaults}};

let config = Config::from_lookup(
    &Environment::Development,
    &Defaults::default(),
    |name| (name == "PORT").then(|| "8000".to_string()),
)
.unwrap();
assert_eq!(config.server.port, 8000);
```

### Health checks

| Route | Meaning |
|---|---|
| `/_ping`, `/_health` | The process is up |
| `/_readiness` | The database, the queue, and any named checks also answer. `503` if not, with `{"ok":false,"failed":[...]}` |

The route knows the database and queue only when axa holds them, which includes
the pool in `AppContext::db` with the `sqlx` feature. Anything else is a named
check, which is an ordinary async closure:

```rust,ignore
ctx.health_checks = Checks::new().add("search", || async { search::ping().await });
```

The route is public, so a caller must not be able to drive load at a dependency.
One answer, passing or failing, is reused for 5 seconds, and callers during a round
of checks wait for it. A check gets 2 seconds, and past that it has failed. Change
these with `Checks::reuse_for` and `Checks::time_limit`. The response carries the
check's name and never its error text, which is only logged. Do not put an address
in a name.

### Command line

`axa::cli::Cli` is a `clap` parser that a service matches on itself:

```text
service start [--worker | --all] [--binding ADDR] [--port PORT]
service db migrate | down [STEPS] | reset | status | truncate
service task [NAME] [KEY:VALUE ...]
service check
```

`axa::cli::parse` reads it, and takes the service's name, version, and description
for `--help` and `--version`:

```rust,no_run
let cli = axa::cli::parse(
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_VERSION"),
    env!("CARGO_PKG_DESCRIPTION"),
);
```

`check` validates the configuration, prints the effective settings with passwords
and other secrets redacted, and exits, non-zero if refused. It connects to nothing,
and it knows only the settings axa reads. `Config::apply_cli` applies the `start`
overrides.

### Tasks

A task is a named job an operator runs by hand:

```rust
use axa::{prelude::*, task::{Tasks, Vars}};

struct Greet;

#[async_trait]
impl Task for Greet {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "greet".to_string(),
            detail: "Say hello".to_string(),
        }
    }

    async fn run(&self, _ctx: &AppContext, vars: &Vars) -> Result<()> {
        println!("hello {}", vars.cli_arg("name")?);
        Ok(())
    }
}

let mut tasks = Tasks::default();
tasks.register(Greet);
```

### Background jobs

A worker is a type with serializable arguments:

```rust
use axa::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Args {
    report_id: u64,
}

struct ReportWorker;

#[async_trait]
impl BackgroundWorker<Args> for ReportWorker {
    fn build(_ctx: &AppContext) -> Self {
        Self
    }

    async fn perform(&self, args: Args) -> Result<()> {
        println!("building report {}", args.report_id);
        Ok(())
    }
}
```

`ReportWorker::perform_later(&ctx, args)` then runs according to `WORKER_MODE`:

| Mode | Behaviour |
|---|---|
| `BackgroundQueue` | Saved to Postgres for a worker process |
| `BackgroundAsync` | Spawned on the current runtime |
| `ForegroundBlocking` | Run before returning. Tests use this |

### The job queue

With the `queue` feature, jobs live in one `axa_queue` table, created on connect.
It needs no broker: Postgres `FOR UPDATE SKIP LOCKED` lets each worker loop claim
the oldest ready job in one statement, so no two loops ever claim the same row.

A job moves from `queued` to `running` to `completed` or `failed`, keeping its
error text on the row. A running job reports in every `heartbeat_interval`, so it
can take as long as it needs, and one not heard from for `stale_minutes` is assumed
lost with its worker. A job that panics fails, and its loop carries on.

A job gets one attempt by default, because a retry repeats its work. Raise
`max_attempts` for the queue, or override `BackgroundWorker::max_attempts` for jobs
written to be repeated, and each retry then waits twice as long as the last. A job
lost with its worker is queued again if it has attempts left and otherwise fails,
so a crash cannot loop forever. Finished rows are deleted after `retention_days`.

`queue::enqueue_on` queues a job on the caller's sqlx transaction, so the job and
the record it is about commit together. `Queue::enqueue` uses its own connection.

**Nothing runs jobs by itself.** A service needs `start --all` or a separate
`start --worker`, and `queue::warn_if_unserved` says so at boot. Queue arguments
are stored in plain text, so never put a secret in them.

### Testing

With the `testing` feature, a service implements `TestApp` to say how it boots
for tests. That is the one trait axa asks a service to implement, and it exists
only for tests.

```rust,ignore
use axa::testing::prelude::*;

#[tokio::test]
async fn health_answers() {
    request::<App, _, _>(|server, _ctx| async move {
        assert_eq!(server.get("/_health").await.status_code(), 200);
    })
    .await;
}
```

## Requirements

- Rust 1.97 or newer, on edition 2024
- PostgreSQL for the `queue` and `sea-orm` features

## Development

```sh
cargo test --all-features
cargo test --features queue,testing
cargo test
cargo clippy --all-targets --all-features -- -D warnings
AXA_TEST_DATABASE_URL=postgres://... cargo test --features queue,testing -- --ignored
```

The last command runs the queue tests against a real Postgres. Those tests have
caught a real race that nothing else did, so run them when you touch the queue.

Read [`AGENTS.md`](AGENTS.md) before changing the crate. A change in behaviour
carries a spec edit, a code edit, and a test edit together.

## Relationship to Loco

To upgrade from an earlier version, read [Upgrading to axa 0.3](help/upgrading-to-0.3/index.md).

To move a project across, follow
[Converting a project from Loco to axa](help/from-loco-into-axa/index.md), and
for the database, [Moving SeaORM code from Loco to axa](help/from-loco-seaorm-into-axa-seaorm/index.md).

axa was extracted from what the Main X Index services relied on in
[Loco](https://loco.rs) (Apache-2.0), and some differences are deliberate:

| Loco | axa |
|---|---|
| A `Hooks` trait drives your app | Your `main.rs` calls plain functions |
| YAML config rendered through a template engine | The environment, with defaults in code |
| Permissive CORS when enabled | Listed origins only in production |
| `x-powered-by` header | None |
| No request timeout or security headers by default | Both on in production; headers everywhere |

The code was written afresh rather than copied, so this crate can keep its own
licence terms.

## License

See [LICENSE.md](LICENSE.md).
