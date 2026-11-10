# axa for Loco users

This page is for someone who knows [Loco](https://loco.rs) well and is meeting axa for
the first time. It says what axa is in Loco's terms, where each Loco concept went, and
which differences are on purpose. To move a real service across, follow
[Converting a project from Loco to axa](../from-loco-into-axa/index.md) and
[Moving SeaORM code from Loco to axa](../from-loco-seaorm-into-axa-seaorm/index.md).
This page does not repeat their steps.

## What axa is

axa was taken from what the Main X Index services used in Loco, and written afresh. Much
will look familiar: `AppContext`, `Routes`, `AppRoutes`, `format::json`,
`BackgroundWorker`, `Task`, and `request::<App, _, _>` keep their names and their shape.
The difference is who is in charge. Loco is a framework: you implement `Hooks`, call
`cli::main::<App, Migrator>()`, and Loco boots the app, builds the middleware, and runs
the command. axa is a library on Axum. Your `main.rs` calls plain functions, in an order
you can read, and gets back an ordinary `axum::Router`. Your handlers are Axum handlers,
and any Axum or Tower crate works with them. axa adds the parts every service needs
around them: the middleware stack, configuration, errors, health routes, the command
line, jobs, and shutdown. What it does not have, you add yourself, or leave out.

## Concept map

| Loco | axa or Axum | Note |
|---|---|---|
| `impl Hooks for App` | Plain functions you call from `main.rs` | Lifecycle hooks are on axa's [must not be added](../../spec/index.md#must-not-be-added) list |
| `cli::main::<App, Migrator>()` and `cargo loco` | `axa::cli::parse`, then your own `match` on `start`, `db`, `task`, and `check` | `cargo loco start` becomes `cargo run -- start` |
| `cargo loco doctor` | `service check` | Validates the settings, prints them with secrets hidden, and connects to nothing |
| Controllers and `Routes` | `Routes` and `AppRoutes`, which build an `axum::Router` | `AppRoutes::with_default_routes()` adds the health routes. `AppRoutes::mounted()` lists `(method, path)` |
| Handlers | Axum handlers. `axa::prelude` brings in `Path`, `Query`, `Json`, `State`, `format`, and the routing functions | `format::json`, `format::html`, `format::redirect` and the rest are still there |
| Extractors, `JsonValidate` | Axum extractors, or `axa::extract::{Json, Path, Query}` | axa's reject a bad request as a problem that quotes nothing. Validation is `Validatable::validate`, called by hand |
| `AppContext` and `SharedStore` | `AppContext`, passed as plain Axum state, with `ctx.shared_store` | It holds the environment, `db`, `config`, `shared_store`, `health_checks`, and with `queue`, `queue_provider`. No mailer, storage, or cache |
| `AppContextBuilder`, `after_context` | Build the `AppContext` yourself, then set its fields | |
| `Initializer` | A function that takes and returns a `Router` | Call them in order, before `middleware::apply` |
| Named middleware stack, `middlewares` hook | One fixed stack, `middleware::apply`, in a [documented order](../../README.md#middleware) | Add your own layers before `apply`. There is no `insert_before`, `replace`, or `delete` |
| `limit_payload` | `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, and `middleware::body_limit(route, bytes)` for one route | |
| Timeout middleware | `REQUEST_TIMEOUT_SECONDS`, 60 seconds in production. `Routes::timeout` and `Routes::add_with_timeout` for one group or route | |
| `RemoteIP` | `axa::client::ClientAddr`, with `TRUSTED_PROXIES` | See [What is different on purpose](#what-is-different-on-purpose) |
| `secure_headers` | Always on. HSTS in production | `HTTP_CONTENT_SECURITY_POLICY`, `HTTP_PERMISSIONS_POLICY`, `HSTS_MAX_AGE_SECONDS` |
| `StaticAssets` and the fallback page | None. Use `tower-http`'s `ServeDir` | An unmatched path answers a `404` problem |
| `loco_rs::Error` | `axa::Error`, sent as an RFC 9457 problem | See [Errors](../../README.md#errors) |
| `config/*.yaml` rendered through Tera | Environment variables, with defaults in code in `Defaults` | No config files. See [Configuration](../../README.md#configuration) |
| `LOCO_ENV` | `APP_ENV` | A release build with no `APP_ENV` is production |
| SeaORM | The `postgres-seaorm` feature (`axa-postgres-seaorm`), or `postgres-sqlx`, `postgres-diesel`, or `postgres-toasty` | Turn on one. `AppContext::db` is that library's pool, from `DB_URL` |
| `loco_rs::schema`, SeaORM migrations | `axa::schema`, and the same `Migrator` | Your `seaql_migrations` history stays |
| `seed`, `dump`, YAML fixtures | None | Write a small helper, or a task |
| `TenantEntity` | None | Postgres row-level security, or the service's own code |
| Background workers | `BackgroundWorker`, with the `queue` feature | The same trait. Postgres, and `axa-mysql-job-queue` and `axa-sqlite-job-queue` as crates of their own. No Redis queue |
| Scheduler | `Queue::every::<Args, Worker>(name, interval, &args)` on `axa::queue::Queue`, or a `Schedule` given to `Queue::schedule` | A fixed interval only, queued once per due time across replicas. No cron, English phrases, or shell commands |
| Tasks | `Task` and `Tasks`, run with `service task NAME key:value` | The same trait |
| JWT auth | `axa-paseto-auth` or `axa-keycloak-auth` (`paseto`, `keycloak` features) | Bearer tokens with ABAC enforcement. No HS256 JWTs |
| Password hashing | None. Use the `argon2` crate | |
| OAuth2 sign-in | None | Keep sign-in in Keycloak, or write it in the service |
| Cookie and server-side sessions | `axa-web-session` (`session` feature), over `axum_session` | Signed and encrypted in production, with form tokens. Nothing stored until `sign_in` |
| Views with Tera | `axa-tera-templates` (`tera` feature), `Templates` and `Page` | Escapes `.html` by default, and a failed render shows nothing |
| `Mailer` | None. Use `lettre` | The [migration guide](../from-loco-into-axa/index.md#what-can-go-wrong-silently) explains the timing risk |
| Storage | None. Use OpenDAL or `object_store` | |
| `Cache` | `axa-redis-cache` (`redis` feature), Redis or `Cache::memory` | Every entry expires. Keep the handle in `shared_store` |
| `Notifier`, webhooks | `axa-transactional-outbox` (`outbox` feature) | It records and relays events. The service brings the publisher |
| `loco_rs::testing` | `axa-test`, as a dev-dependency | `request::<App, _, _>` stays. Implement `TestApp` |
| `/_ping`, `/_health`, `/_readiness` | `/_health/live`, `/_health/ready`, `/_health/startup` | See [Health routes](../../spec/health/index.md) |
| `on_shutdown` | Code after `server::serve` returns | `SIGTERM` and Ctrl-C drain, then wait. `server::Shutdown` lets a stream end itself |
| Logger | `axa::logger::init`, with `LOG_LEVEL`, `LOG_FORMAT`, `LOG_FILTER` | Every line carries the request id and route template, never the path or query |
| Metrics | `axa-prometheus-metrics` and `axa-opentelemetry` | |
| `cargo loco generate` | None | Code generation is on the must-not-be-added list |

## A first service

This is the service `cargo loco new` gives you with no database, cut down to one route.
In Loco it spans a controller, `app.rs`, `main.rs`, and a YAML file per environment:

```rust,ignore
// src/controllers/home.rs
use loco_rs::prelude::*;

async fn current() -> Result<Response> {
    format::json(serde_json::json!({ "hello": "world" }))
}

pub fn routes() -> Routes {
    Routes::new().prefix("/api").add("/", get(current))
}

// src/app.rs
pub struct App;

#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    async fn boot(mode: StartMode, environment: &Environment, config: Config) -> Result<BootResult> {
        create_app::<Self>(mode, environment, config).await
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes().add_route(controllers::home::routes())
    }

    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        Ok(())
    }

    fn register_tasks(_tasks: &mut Tasks) {}
}

// src/bin/main.rs
#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    cli::main::<App>().await
}
```

```yaml
# config/development.yaml, and one each for test and production
server:
  port: <%= get_env(name="PORT", default="5150") %>
  binding: <%= get_env(name="BINDING", default="localhost") %>
```

In axa the same service is one file, and every step is in it:

```rust,ignore
use axa::{
    Environment, Result,
    config::{Config, Defaults},
    logger,
    prelude::*,
    server,
};

async fn current() -> Result<Response> {
    format::json(serde_json::json!({ "hello": "world" }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let defaults = Defaults {
        // An empty name means this service has no database.
        database: "",
        // Loco's port, so proxies and probes need no change. axa's own is 1241.
        port: 5150,
        ..Defaults::default()
    };
    let config = Config::from_env(&environment, &defaults)?;
    logger::init(env!("CARGO_CRATE_NAME"), &config.logger);

    let server_config = config.server.clone();
    let ctx = AppContext::new(environment, (), config);

    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/", get(current)))
        .into_app(ctx)?;

    server::serve(router, &server_config).await
}
```

```toml
[dependencies]
axa = "0.11"
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

It serves `/api`, the three health routes, and the standard middleware, and stops
cleanly on `SIGTERM`. `PORT=8000 cargo run` changes the port. A real service adds
`axa::cli::parse` for `start`, `db`, `task`, and `check`, and the migration guide shows
that `App::run` [in full](../from-loco-into-axa/index.md#3-turn-hooks-into-plain-functions).

## What is different on purpose

Each of these comes from a principle in [the spec](../../spec/index.md#3-principles).

**You can read the control flow.** Loco's `Hooks` trait has about twenty methods that
Loco calls in its own order. axa has no hooks, no route macros, and no plugin system
(P-1). What `boot`, `after_routes`, `initializers`, and `on_shutdown` did becomes code
in your `main.rs`, in the order it runs. It is longer, and an auditor reads it once.

**One middleware order.** Loco lets YAML and the `middlewares` hook switch layers on and
off and move them by name. axa has one documented order. You add your own layers, such
as an auth guard, to the router before `middleware::apply`, so they sit inside the
stack. A layer added after `apply` sits outside it: its refusals get no request id,
security headers, or CORS headers. This compiles, so test a refusal's headers.

**Configuration is the environment.** There are no config files and no template engine
in front of them. Defaults live in code, in `Defaults`, and each variable overrides one.
Production refuses to boot on a missing `DB_URL`, a `DB_URL` without TLS, `*` in
`CORS_ALLOWED_ORIGINS`, or any setting that is set but invalid, such as `HTTP_HSTS=tru`,
and names the variable (P-3). In Loco, `get_env(default="")` could quietly give an empty
secret. Any environment name axa does not know, such as `staging`, gets production's
strict behaviour, so check guards with `ctx.environment.is_production_like()`.

**Errors never send internal text.** Every error is an `application/problem+json` body
with a `code`. A `500` carries no detail, and the real error is only logged (P-4). A
broken unique or foreign key constraint answers `409`, a broken check or not-null
constraint `422`, and a database that cannot serve `503`. Axum's own refusals, such as a
`405` or a bad `Json` body, become problems too.

**Client addresses come only from proxies you name.** Loco's `RemoteIP` now trusts one
configured header source, with no chain of proxies. axa reads `X-Forwarded-For` from the
right, only through the addresses or CIDR ranges in `TRUSTED_PROXIES`, and ignores it
when that is unset. A handler reads the result with `axa::client::ClientAddr`. Behind a
load balancer, list it, or every client looks like the load balancer, and
`axa-request-rate-limit` warns that they all share one limit.

**Safe defaults are not options.** Security headers go on every response, HSTS and a
strict `content-security-policy` in production, a 60 second timeout, no CORS in
production until origins are listed, and no `x-powered-by` header. Where a safer
behaviour replaced a weaker one, the weaker one is gone rather than kept as a setting
(P-10). A service that serves pages sets `HTTP_CONTENT_SECURITY_POLICY`.

**Health has three routes, one per probe.** Liveness asks nothing. Readiness asks the
database, the queue, and your named checks in `ctx.health_checks`, and refuses from the
moment shutdown begins. Startup asks the database until it answers, then passes for good. Answers
are reused for a few seconds, so a public route cannot drive load at the database. None
of them should ask a third party.

**Destroying data is refused by default.** `db down`, `db reset`, and `db truncate` run
only in development or test, with `AXA_DESTROY_DATA=true`, and the command line refuses
them before your code runs.

## Where axa is safer, and where it does less

Loco is far broader than axa, and much of that breadth is useful. The
[comparison](../../spec/compare/loco/index.md) lists it in detail. In short:

**Where axa chose a stricter design:**

- Every cache entry expires, and a Redis failure falls back to the loader. Loco's cache
  allows entries with no expiry and has a database driver.
- Jobs live in the same database as the records they are about, so `queue::enqueue_on`
  commits a job with its record. axa has no Redis queue for that reason.
- Jobs report a heartbeat, a job lost with its worker is retried or failed, not looped,
  and `queue::cancel` and `queue::retry` are acts of a named person, kept as records.
- Bearer auth is PASETO or Keycloak, not JWTs signed with a shared HMAC key.
- The scheduler runs jobs, never shell text from configuration.
- Session data is signed and encrypted in production, and nothing is stored for a
  visitor who has not signed in.
- Tera pages escape HTML by default, and a failed render shows no template source.

**What Loco has that axa does not, and what to use meanwhile:**

| Loco has | Use meanwhile |
|---|---|
| `Mailer` with templates and a test stub | `lettre`, sent from a detached `tokio::spawn`, as the migration guide explains |
| Storage drivers for disk, S3, Azure, and GCS | OpenDAL or `object_store` |
| Cron and English schedules | `Queue::every`, or a Kubernetes `CronJob` that runs `service task NAME` |
| Signed webhooks and Slack notifications | The outbox, with a publisher of your own |
| OAuth2 sign-in, argon2 helpers | Keycloak, or the `argon2` crate and your own routes |
| Multi-tenancy for SeaORM | Postgres row-level security |
| Seeding and fixtures | A small helper using `Entity::insert(model).exec(db)` |
| A fresh test database per test | One test database, reset in `boot_test`, with tests run one at a time |
| HTML selector assertions, snapshot redaction | `scraper` and `insta` in your own tests |
| Static file middleware | `tower-http`'s `ServeDir` |
| Generators | Write the code, or copy it from a service you trust |

axa also has things Loco does not: idempotency keys, a transactional outbox, signed
cursor pagination, `Accepts-version` negotiation, OpenAPI drift checks, and
connection-level timeouts.

## Moving a service across

Do it in this order. The two guides have the detail.

1. **Get a baseline.** Run every test, including those against a real Postgres, and
   write the counts down.
2. **Keep SeaORM at first.** Use the `postgres-seaorm` feature, so the first change is only about
   Loco. Moving to sqlx is a second change.
3. **Replace `Hooks` with `App::run`**, and initializers with functions that take and
   return a `Router`. Add your layers before `middleware::apply`.
4. **Replace `config/` with variables.** Move each YAML value to `Defaults` or a
   variable: `DB_URL` (not `DATABASE_URL`), `APP_ENV` (not `LOCO_ENV`),
   `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, `CORS_ALLOWED_ORIGINS`, `TRUSTED_PROXIES`, and
   `HTTP_CONTENT_SECURITY_POLICY` for a service that serves pages. Keep Loco's port in
   `Defaults`, or move your proxies to the new one.
5. **Move the probes.** Liveness to `/_health/live`, readiness to `/_health/ready`, and
   optionally startup to `/_health/startup`. Update load balancers and uptime checks. On
   Kubernetes, set `SHUTDOWN_DRAIN_SECONDS` a little above the time the cluster takes to
   remove a pod.
6. **Drain `pg_loco_queue`.** axa uses `axa_queue`, and moves no jobs.
7. **Test the real binary.** Run `APP_ENV=production cargo run -- check` with the real
   settings, start it, and check that it refuses bad settings, sends the security
   headers on a refusal, answers the three health routes, and exits cleanly on
   `SIGTERM`.

The silent risks, such as a guard still reading `LOCO_ENV`, or a router merged after the
middleware, are listed in
[What can go wrong silently](../from-loco-into-axa/index.md#what-can-go-wrong-silently).
Read that before you start.

## Further reading

- [Converting a project from Loco to axa](../from-loco-into-axa/index.md), the step by
  step
- [Moving SeaORM code from Loco to axa](../from-loco-seaorm-into-axa-seaorm/index.md)
- [The comparison with Loco](../../spec/compare/loco/index.md), from Loco's source
- The README: [Quick start](../../README.md#quick-start),
  [Middleware](../../README.md#middleware), [Errors](../../README.md#errors),
  [Configuration](../../README.md#configuration),
  [Health checks](../../README.md#health-checks),
  [Command line](../../README.md#command-line), and
  [Relationship to Loco](../../README.md#relationship-to-loco)
- [The specification](../../spec/index.md), and its [principles](../../spec/index.md#3-principles)
- [Health routes](../../spec/health/index.md)
- [Background jobs and schedules](../background-jobs/index.md)
- [The transactional outbox](../transactional-outbox/index.md)
- [Idempotency keys](../idempotency-keys/index.md)
- [Upgrading to axa 0.11](../upgrading-to-0.11/index.md)
