# Axa

**Axa** is a shared library for web services built with [Axum](https://docs.rs/axum),
that provides a middleware stack, health checks, error responses, configuration
helpers, route tables, logging capabilities, background jobs, database queues, and
graceful shutdown.

## A library, not a framework

`AppContext` is a plain struct, passed as ordinary Axum state, and `AppRoutes`
builds an ordinary `axum::Router`. Any piece can be read, replaced, or left out on
its own.

## Installing

Add it from crates.io:

```toml
[dependencies]
axa = "0.12"
```

### Always included

- [`axa-environment`](crates/axa-environment): which environment a service runs in. It has no dependencies, so a migration crate can read the environment the same way without depending on `axa`
- [`axa-problem`](crates/axa-problem): RFC 9457 problem details, as `axa::problem`

### Data

Each of these gives the same things for its library, so a service can move between
them: `AppContext::db` becomes its pool, `axa::db::connect` opens it from configuration,
`/_health/ready` checks it, and each has `migrate`, `pending`, `truncate` and
`dangerously_recreate` for tests, `is_unique_violation`, and `quote_identifier`. With
any of them, `axa::model` and `axa::validation` give model errors and validation, and a
broken constraint answers `409` or `422`.

Each crate, and the `axa` feature that turns it on, is named for its database and
library:

| | sqlx | SeaORM | Diesel | Toasty |
|---|---|---|---|---|
| Postgres | [`axa-postgres-sqlx`](crates/axa-postgres-sqlx) | [`axa-postgres-seaorm`](crates/axa-postgres-seaorm) | [`axa-postgres-diesel`](crates/axa-postgres-diesel) | [`axa-postgres-toasty`](crates/axa-postgres-toasty) |
| MySQL | [`axa-mysql-sqlx`](crates/axa-mysql-sqlx) | [`axa-mysql-seaorm`](crates/axa-mysql-seaorm) | [`axa-mysql-diesel`](crates/axa-mysql-diesel) | [`axa-mysql-toasty`](crates/axa-mysql-toasty) |
| SQLite | [`axa-sqlite-sqlx`](crates/axa-sqlite-sqlx) | [`axa-sqlite-seaorm`](crates/axa-sqlite-seaorm) | [`axa-sqlite-diesel`](crates/axa-sqlite-diesel) | |

- **sqlx** has no ORM: a service writes its own SQL. `AppContext::db` is the pool.
- **SeaORM** adds models, schema helpers, and an easy path from Loco. `AppContext::db`
  is a `DatabaseConnection`.
- **Diesel** needs no C database client library: no `libpq` or `libmysqlclient`.
  `AppContext::db` is a Diesel pool.
- **Toasty** is before 1.0, and its migrations go forward only. `AppContext::db` is a
  `toasty::Db`. There is no SQLite Toasty crate, as its SQLite driver needs a newer
  SQLite library than sqlx allows in one build.

MySQL takes TLS from the URL's `ssl-mode`, which production requires, in every MySQL
crate alike. The SQLite crates build `libsqlite3-sys` 0.37, which has RUSTSEC-2026-0459,
so they are not published until sqlx raises its cap on it.

Turn on one. With more than one, Postgres wins, then MySQL, then SQLite, and for each,
sqlx, then SeaORM, Diesel, and Toasty.

### Authentication

- [`axa-paseto-auth`](crates/axa-paseto-auth): PASETO bearer authentication and ABAC enforcement. Enforcement is on unless turned off
- [`axa-keycloak-auth`](crates/axa-keycloak-auth): Keycloak bearer authentication and ABAC enforcement, standing alone so it can be audited alone

### Templates

- [`axa-maud-templates`](crates/axa-maud-templates): Maud compile-time HTML templates with safe escaping and generic errors, checked by the compiler
- [`axa-tera-templates`](crates/axa-tera-templates): Tera templates with safe escaping and generic errors, loaded from files

### Other optional features

- [`axa-postgres-job-queue`](crates/axa-postgres-job-queue): the Postgres job queue, worker pool, and schedules
- [`axa-tracing`](crates/axa-tracing): redaction, request tracking across spawned tasks and peers, and diagnostics for blocked or stalled async code
- [`axa-redis-cache`](crates/axa-redis-cache): a Redis cache with mandatory expiry
- [`axa-openapi-docs`](crates/axa-openapi-docs): OpenAPI documents, a pinned Swagger UI, route listing, and drift checks
- [`axa-opentelemetry`](crates/axa-opentelemetry): OTLP export of traces and request metrics, off until an endpoint is set
- [`axa-web-session`](crates/axa-web-session): web sessions over `axum_session`, signed and encrypted in production, with form tokens. Nothing is stored for a visitor until they sign in
- [`axa-transactional-outbox`](crates/axa-transactional-outbox): a transactional outbox and relay on Postgres that keeps a record of what it delivered and refused, with the broker client left in the service. Brings in `axa-postgres-job-queue`
- [`axa-cursor-pagination`](crates/axa-cursor-pagination): signed cursor pages with `Link: rel="next"` and no total, and `LIKE` escaping
- [`axa-http-client`](crates/axa-http-client): an outbound HTTP client that refuses private, loopback, link-local, and metadata addresses, after DNS and on every redirect
- [`axa-webhooks`](crates/axa-webhooks): Standard Webhooks signing and verifying, HMAC-SHA256 and Ed25519, several keys while one rotates, and a verifying extractor
- [`axa-prometheus-metrics`](crates/axa-prometheus-metrics): Prometheus text exposition, and request durations by route template
- [`axa-request-rate-limit`](crates/axa-request-rate-limit): request limits per client or per key, and failed-login lockout, counted in one process
- [`axa-idempotency-key`](crates/axa-idempotency-key): `Idempotency-Key` handling over Postgres, so a retried write gets its first answer back. [Where the layer goes](help/idempotency-keys/index.md)
- [`axa-accepts-version-negotiation`](crates/axa-accepts-version-negotiation): `Accepts-version` negotiation

For tests, add [`axa-test`](crates/axa-test) as a dev-dependency. It has request-level
test helpers, built on `axum-test`, and is not a feature of `axa`.

For MySQL, [`axa-mysql-job-queue`](crates/axa-mysql-job-queue) is the job queue, worker
pool, and schedules on MySQL 8.4, and for a service on one machine,
[`axa-sqlite-job-queue`](crates/axa-sqlite-job-queue) is the same on a SQLite file. Each
has the same API as `axa-postgres-job-queue`, and is a crate on its own, not yet a feature
of `axa`.

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

It listens on port 1241 and serves `/api/hello`, `/_health/live`, `/_health/ready`,
and `/_health/startup` under the standard middleware. It stops cleanly on `SIGTERM` or
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
router. A service with layers of its own, such as an auth guard, adds them first and
applies the stack last, because a layer added afterwards sits outside it. Its responses
get no request id, security headers, or CORS headers, it has no panic or timeout
protection, and it reads response bodies compressed:

```rust,ignore
let router = routes.into_router(ctx.clone()).layer(from_fn(require_auth));
let router = axa::middleware::apply(router, &ctx.config.http)?;
```

From the outside in:

| Layer | Behaviour |
|---|---|
| Request id | Keeps a caller's `x-request-id` after removing unsafe characters, or mints a UUID. Always returned |
| Tracing | Logs each finished request, and puts the request id, method, and route template on every log line written while serving it. The real path and query string are left out, because either can hold a token or personal data |
| Panic catching | Turns a handler panic into a `500` |
| Security headers | `x-content-type-options`, `x-frame-options`, `referrer-policy`, `cross-origin-opener-policy` and `cross-origin-resource-policy` of `same-origin`, `permissions-policy`, and optionally HSTS. A handler's own value wins |
| No store | `cache-control: no-store` on a response that sets a cookie, unless its handler said how to cache it |
| CORS | Off unless origins are configured |
| Timeout | Answers `408` after `REQUEST_TIMEOUT_SECONDS`. `Routes::timeout` and `Routes::add_with_timeout` set a route's own, and `Http::timeout_overrides` sets one by path suffix |
| Not modified | Answers `304` to a `GET` or `HEAD` holding the `ETag` of its answer. See [Conditional requests](#conditional-requests) |
| Compression | Gzip, Brotli, and Zstd when enabled, but never a response that sets a cookie or is `no-store`, which compression can leak (BREACH) |
| Body limit | Answers `413` over `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, for the extractors and for a handler that reads the body itself. `middleware::body_limit(route, bytes)` raises it on one route |

### Conditional requests

`axa::conditional` follows RFC 9110. The standard stack answers `304` to a `GET` or
`HEAD` that holds the `ETag` of its answer, whoever set the tag, so a handler only sets
one. An `ETag` goes on a response like any header. Leave a timestamp out of it, or the
tag moves every time:

```rust
use axa::{conditional::ETag, prelude::*};
use serde_json::json;

async fn dashboard() -> Result<Response> {
    let payload = json!({ "open": 3 });
    let tag = ETag::weak_of(&payload)?;
    let mut body = payload;
    body["as_of"] = json!("2026-11-07T09:00:00Z");
    Ok((tag, format::json(body)?).into_response())
}
```

`ETag::weak` and `ETag::strong` take text of your own, such as a version. For a tag from
the body, add `conditional::etag` to a router or a route, before `apply` as with any
layer:

```rust
use axa::{conditional, prelude::*};
use axum::{Router, middleware::from_fn};

# async fn list() -> &'static str { "[]" }
let catalogue: Router = Router::new()
    .route("/catalogue", get(list).layer(from_fn(conditional::etag)));
```

It hashes a `GET`'s `200` and gives it a weak tag, the same on every replica and for
every encoding. It reads only a body of exact known size up to `conditional::MAX_BODY`
(8 MB), so a stream or server-sent events pass through unread. It leaves alone an answer
that has an `ETag` or says `no-store`, and a `HEAD`, whose body Axum has already dropped.
It is opt-in because a body with a timestamp in it gains nothing, and because hashing
what is marked `no-store` would put a hash of personal data on an answer nobody can
revalidate. Put it outside what shapes the body, such as paging, and outside what sets
`Cache-Control`.

To refuse a stale write, read the record, then ask, once the caller is authorised:

```rust
use axa::{conditional::{self, ETag}, prelude::*};
use axum::http::HeaderMap;

async fn rename(headers: HeaderMap) -> Result<Response> {
    # let version = 7;
    conditional::require_if_match(&headers, &ETag::strong(&format!("v{version}"))?)?;
    format::empty()
}
```

`require_if_match` answers `428` with no `If-Match`, and `if_match` lets that through.
Both answer `412` to a stale tag, and `if_match_holds` gives the answer alone for a
service with a refusal of its own. `If-Match` compares strongly, as RFC 9110 requires, so
a weak tag never passes it and a tag meant for it must be strong. The check and the write
are two steps, so close the gap with a lock on the row or `UPDATE ... WHERE version = $1`.

A `304` comes after the handler has run, so it saves bytes and not the query. It is made
from the answer the caller would have had, so a refused caller gets the refusal and a tag
says nothing about what they may not see. The hash is std's `DefaultHasher`, which catches
accidents and not an adversary: a tag defending against someone who writes the data should
be a version counter or a real digest.

### Errors

Handlers return `Result<T>` using `axa::Error`, which maps to a status and an
[RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) problem, with the media type
`application/problem+json`. Successes stay bare JSON.

| Error | Status | `code` | `detail` |
|---|---|---|---|
| `Error::NotFound` | `404` | `not_found` | Fixed text |
| `Error::Unauthorized(_)` | `401` | `unauthorized` | Fixed text; yours is only logged |
| `Error::BadRequest(_)` | `400` | `bad_request` | Your text |
| `Error::CustomError(status, detail)` | Yours | Yours | Yours |
| Anything else | `500` | `internal_server_error` | None; the real error is only logged |

So a failed database call cannot put a hostname or a query in a client's
response. The `not_found`, `unauthorized`, and `bad_request` functions build
these for you.

```json
{"type":"about:blank","title":"Bad Request","status":400,"detail":"name is required","code":"bad_request"}
```

`code` is the machine-readable reason. Field errors go in an `errors` member, which
`ErrorDetail::errors` sets. The standard stack makes Axum's own refusals problems too:
an unmatched path's `404`, a wrong method's `405`, and an extractor's rejection. It
treats any error sent as plain text or with no `content-type` the same way and drops
the text, so a handler that has something to say returns an `Error`.

### Configuration

There are no config files. A service states its defaults in code, and the
environment overrides them, so what a deployment runs with is visible in one
place.

| Variable | Effect |
|---|---|
| `APP_ENV` | `development`, `test`, or `production`. Any other name gets production's stricter behaviour. With it unset, a debug build is development and a release build is production |
| `PORT`, `BINDING`, `HOST` | Listening port, address, and public URL |
| `DB_URL` | Connection string. Required in production for a service with a database, where it must set `sslmode` to `require`, `verify-ca`, or `verify-full` unless it stays on this machine. `DB_URL_FILE` may name a file holding it instead |
| `DB_ALLOW_INSECURE` | Let a production service reach its database without demanding TLS, as on a network that encrypts its own traffic |
| `DB_MIN_CONNECTIONS`, `DB_MAX_CONNECTIONS` | Pool size |
| `DB_CONNECT_TIMEOUT`, `DB_IDLE_TIMEOUT` | Pool timeouts, in milliseconds. 5 seconds and 10 minutes by default, or a service's `Defaults` |
| `DB_LOGGING`, `DB_AUTO_MIGRATE` | Statement logging, and migrating at boot. A service sharing a database it does not own sets `Defaults::auto_migrate` to `false` |
| `DB_STATEMENT_TIMEOUT_MS`, `DB_APPLICATION_NAME`, `DB_READ_ONLY` | Postgres only: a `statement_timeout` on every connection, `0` by default for none; the `application_name` that `pg_stat_activity` shows, the database name stem by default; and read-only transactions, for a replica |
| `LOG_ENABLE`, `LOG_LEVEL`, `LOG_FORMAT`, `LOG_FILTER` | Logging. `RUST_LOG` wins over all of them |
| `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT` | Largest accepted request body |
| `HTTP_COMPRESSION`, `HTTP_HSTS` | Response compression and HSTS |
| `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS` | How long a client has to send a request's headers, once it connects or starts another request. 30 by default, and `0` allows for ever |
| `KEEP_ALIVE_TIMEOUT_SECONDS` | How long an idle connection is kept for a next request. Keep it above any load balancer's idle timeout. 75 by default, and `0` keeps it for ever |
| `SEND_STALL_TIMEOUT_SECONDS` | How long a client that has stopped reading may hold up a response before it is dropped. Any write that goes through starts this again. 60 by default, and `0` waits for ever |
| `MAX_CONNECTIONS` | The most connections open at once. 10,000 by default, and more wait to be accepted |
| `TRUSTED_PROXIES` | Comma-separated addresses or CIDR ranges of the proxies whose `X-Forwarded-For` entries are believed. Unset ignores the header |
| `SHUTDOWN_TIMEOUT_SECONDS` | How long, once told to stop, to wait for requests and jobs. 25 by default, and `0` waits for as long as they take. For requests, it counts from the end of the drain |
| `SHUTDOWN_DRAIN_SECONDS` | How long, once told to stop, to keep accepting while `/_health/ready` answers `503`, so the orchestrator takes the service out of rotation first. 0 by default. On Kubernetes, a little more than the cluster takes to remove a pod |
| `HSTS_MAX_AGE_SECONDS` | How long browsers keep to HTTPS. One year by default |
| `HTTP_CONTENT_SECURITY_POLICY` | The `content-security-policy`. Production sends `default-src 'none'; frame-ancestors 'none'` unless this is set, and sends none if it is empty |
| `HTTP_PERMISSIONS_POLICY` | The `permissions-policy`. Unless this is set, it turns off the camera, microphone, location, payment, USB, sensors, and ad topics for every origin, and it sends none if it is empty |
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

There is one route for each probe Kubernetes gives a container. Point each probe at its
own route:

| Route | Probe | A failure means | It asks |
|---|---|---|---|
| `/_health/live` | Liveness | Kill and restart the container | Nothing: it answers `200` once the server is running |
| `/_health/ready` | Readiness | Take the pod out of the Service, so it gets no traffic | The database and queue Axa holds, and your named checks. `503` if one fails, as a problem whose `failed` names them |
| `/_health/startup` | Startup | Not booted yet: hold off the other two | The database Axa holds. Once it passes, it passes for good |

```yaml
livenessProbe:
  httpGet: { path: /_health/live, port: 1241 }
readinessProbe:
  httpGet: { path: /_health/ready, port: 1241 }
startupProbe:
  httpGet: { path: /_health/startup, port: 1241 }
  failureThreshold: 30
  periodSeconds: 2
```

None of them asks a third party. If a payment provider goes down, restarting the
container does not bring it back, and taking the pod out of the Service gives every
visitor a `503`, where a ready pod could still serve cached pages or show a "payments
are offline" banner. So add a named check only for something the service owns, such as
its Redis or search index. A named check is an ordinary async closure:

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

`parse` refuses `db down`, `db reset`, and `db truncate`, because they destroy data.
They run only in development or test, with `AXA_DESTROY_DATA=true`, and the process
exits non-zero before your code runs. `--environment` cannot lift this: a production
process stays refused whatever the flag says. `Cli::parse()` refuses them too.

`check` validates the configuration, prints the effective settings with passwords
and other secrets redacted, and exits, non-zero if refused. It connects to nothing,
and it knows only the settings Axa reads. `Config::apply_cli` applies the `start`
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

With the `queue` feature, jobs live in one `axa_queue` table. `Queue::connect` creates
it, and `Queue::connect_existing` changes no schema, for a role that may not: a
migration then runs `queue::SETUP_STATEMENTS`.
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

A process runs `workers` jobs at once, 2 by default, and `Queue::limit` holds one job
type below that. A job may be delayed, given a priority, or given a time limit, and has
none unless it is given one. `Queue::job` and `counts` say where a job is.
`queue::cancel` and `queue::retry` change it, as an act of a named person for a
reason, kept in the insert-only `axa_job_acts` in the caller's transaction, and
`Queue::acts` reads them back. A run that lost its claim is stopped, and its job waits
in `needs_review` for a person. `Queue::every` queues a job each interval, once per due
time across replicas, with no drift, one job for times missed, and no stacking, and a
schedule removed from code is archived, not deleted. See
[Background jobs and schedules](help/background-jobs/index.md).

**Nothing runs jobs by itself.** A service needs `start --all` or a separate
`start --worker`, and `queue::warn_if_unserved` says so at boot. Queue arguments
are stored in plain text, so never put a secret in them.

### The outbox

With the `outbox` feature, an event is a record, written with `outbox::write` on the
caller's transaction, so it commits or rolls back with the change it describes. A relay
then decides whether each event may go, publishes it, and records what happened, so a
crash between the commit and the publish loses nothing. `Outbox::run` takes three
closures: the decision, run in the claim's transaction, the publisher, so the broker
client stays in the service, and the record, run in each outcome's transaction, so the
service's audit row commits with it. A service with no rule and no audit passes
`decide::deliver_all` and `record::nothing`.

Events and attempts are only ever inserted, and the database's triggers refuse every
change, so what was delivered, refused, and failed stays on record. A refusal is final.
A hold charges nothing. Nothing is swept: a payload that must go is erased by a person,
with `outbox::erase`, and the erasure is kept. A changed row is never published.

Delivery is at least once, so a consumer must dedupe on `event_id`, which
`outbox::inbox::seen` does. Several relays can run at once, and each event normally goes
out once. Events with one topic and key go out in order, unless a topic is given
`Order::None`. A broker that is down charges no event, and one it keeps refusing ends as
`failed` and holds up nothing else.

**Nothing relays by itself.** See the
[guide](help/transactional-outbox/index.md) for what a service writes. A payload is at
most 64 KiB by default, may be checked against a list of denied fields, and with
`outbox-encryption` is sealed at rest. Prefer an id to a snapshot.

### Testing

With [`axa-test`](crates/axa-test) as a dev-dependency, a service implements `TestApp` to say how it boots
for tests. That is the one trait Axa asks a service to implement, and it exists
only for tests.

```rust,ignore
use axa_test::prelude::*;

#[tokio::test]
async fn health_answers() {
    request::<App, _, _>(|server, _ctx| async move {
        assert_eq!(server.get("/_health/live").await.status_code(), 200);
    })
    .await;
}
```

## Requirements

- Rust 1.97 or newer, on edition 2024
- PostgreSQL for the `queue`, `outbox`, and `postgres-seaorm` features

## Development

```sh
cargo test --workspace --all-features
cargo test --features queue
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
AXA_TEST_DB_URL=postgres://... cargo test --features queue -- --ignored
AXA_TEST_DB_URL=postgres://... cargo test -p axa-postgres-job-queue -- --ignored --test-threads=1
AXA_TEST_MYSQL_URL=mysql://... cargo test -p axa-mysql-job-queue -- --ignored --test-threads=1
```

The last two commands run the queue tests against a real Postgres and MySQL. Those tests have
caught a real race that nothing else did, so run them when you touch the queue.
The full list that CI runs is in [section 5 of the spec](spec/index.md#5-verification).

AI agents have two skills: [`axa-skill`](skill/axa-skill/SKILL.md) for building a
service with Axa, and [`axa-maintainer-skill`](skill/axa-maintainer-skill/SKILL.md) for
working on Axa itself.

Read [`AGENTS.md`](AGENTS.md) before changing the crate. A change in behaviour
carries a spec edit, a code edit, and a test edit together.

## Coming from another framework

Each of these explains Axa in the terms of a framework you know: a map of its concepts
onto Axa and Axum, a first service written both ways, what is different on purpose, and
how to move a service across.

- [axa for Loco users](help/axa-for-loco-users/index.md)
- [axa for actix-web users](help/axa-for-actix-users/index.md)
- [axa for Rocket users](help/axa-for-rocket-users/index.md)
- [axa for Poem users](help/axa-for-poem-users/index.md)
- [axa for Topcoat users](help/axa-for-topcoat-users/index.md)

## Relationship to Loco

To upgrade from 0.11, read [Upgrading to Axa 0.12](help/upgrading-to-0.12/index.md), from 0.10,
[Upgrading to Axa 0.11](help/upgrading-to-0.11/index.md), and
from earlier versions, [Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md).

To move a project across, follow
[Converting a project from Loco to Axa](help/from-loco-into-axa/index.md), and
for the database, [Moving SeaORM code from Loco to Axa](help/from-loco-seaorm-into-axa-seaorm/index.md).

Axa was extracted from what the Main X Index services relied on in
[Loco](https://loco.rs) (Apache-2.0), and some differences are deliberate:

| Loco | Axa |
|---|---|
| A `Hooks` trait drives your app | Your `main.rs` calls plain functions |
| YAML config rendered through a template engine | The environment, with defaults in code |
| Permissive CORS when enabled | Listed origins only in production |
| `x-powered-by` header | None |
| No request timeout or security headers by default | Both on in production; headers everywhere |
| `If-None-Match` compared as one exact string | Weak comparison, lists, and `*`, as RFC 9110 has it |

The code was written afresh rather than copied, so this crate can keep its own
licence terms.

## License

See [LICENSE.md](LICENSE.md).
