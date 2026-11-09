# Converting a project from Loco to axa

This guide takes a working [Loco](https://loco.rs) service to plain Axum plus
[axa](../../README.md). It comes from converting 13 production-shaped services,
and it calls out the mistakes those conversions actually made.

Most of the work is mechanical. The danger is not in what fails to compile. It
is in what compiles, passes its tests, and is quietly weaker than before, so
read [What can go wrong silently](#what-can-go-wrong-silently) before you start.

## Before you start

**Get a baseline.** Run the whole test suite on the unconverted code, including
anything that needs a real database, and write down the counts. If a test fails
after conversion you must be able to say whether you caused it.

**Keep your database tests running.** Unit tests are not enough. In our
conversions, four services had every unit test passing and 13 failures between
them on real Postgres.

**Moving SeaORM code?** [Moving SeaORM code from Loco to axa](../from-loco-seaorm-into-axa-seaorm/index.md)
covers the database half in detail.

**Convert one service first**, ideally the one with the least database code.
Keep SeaORM for now by using axa's `sea-orm` feature, so this change is only about
Loco. Moving from SeaORM to sqlx is a separate change.

## What changes

| Loco | axa |
|---|---|
| `impl Hooks for App` | Plain functions on `App` |
| `cli::main::<App, Migrator>()` | `App::run(cli::parse(...))`, written out in your own code |
| `config/*.yaml` | Environment variables, with defaults in code |
| `LOCO_ENV` | `APP_ENV` (`LOCO_ENV` is still read by axa) |
| `Initializer` | A function that takes and returns a `Router` |
| `use loco_rs::prelude::*` | `use axa::prelude::*` |
| `ctx.shared_store`, `AppContext`, `Routes` | The same names, with the same shape |
| `BackgroundWorker`, `Task` | The same traits. `Queue::register` is now synchronous |
| `pg_loco_queue` | `axa_queue` |
| `loco_rs::schema` | `axa::schema` |
| `request::<App, _, _>` | The same, from `axa_test::prelude` |
| `cargo loco start` | `cargo run -- start` |
| `cargo loco doctor` | `cargo run -- check`, which reads the settings and connects to nothing |
| `start --server-and-worker` | `start --all`. The old spelling is still accepted |
| `app_routes().collect()` | `app_routes().mounted()`, which lists each `(method, path)`. `axa::openapi::drift` replaces a hand-written coverage test |
| `ctx.cache` | Nothing in `AppContext`. Use `axa-redis-cache`, and keep its handle in `ctx.shared_store` |
| `Mailer` | Use [`lettre`](https://docs.rs/lettre) directly |
| `hash::hash_password` | Use [`argon2`](https://docs.rs/argon2) directly |
| `model::query::condition()` | The same, as `axa::query::condition()` |
| `Validatable` | The same, as `axa::validation::Validatable` |
| `Authenticable` | Plain methods on your model |
| `cli::playground` | Delete it |

## Step by step

### 1. Update the manifest

Remove `loco-rs` and add axa, pinned to a revision:

```toml
[dependencies]
axa = { git = "https://github.com/publicprojects-io/axa", rev = "<commit>", features = ["sea-orm"] }
clap = { version = "4", features = ["derive"] }

[dev-dependencies]
axa-test = { git = "https://github.com/publicprojects-io/axa", rev = "<commit>" }
axum-test = "21"
```

Add the `queue` feature if the service has background workers. A service with no
database of its own needs no features, and can then delete `sea-orm`, its
migration crate, and its models.

Do the same in `migration/Cargo.toml` if it depends on `loco-rs`:

```toml
axa = { git = "https://github.com/publicprojects-io/axa", rev = "<commit>", features = ["sea-orm"] }
```

**Loco was enabling features for you.** Expect errors like `Uuid: Serialize is
not satisfied`. Loco's dependency tree switched these on, and your crate now has
to ask for them. The ones we hit were `uuid`'s `serde`, `tokio`'s `macros`, and
`validator`'s `derive`.

**Match `axum-test` to axa.** A crate listing its own older `axum-test` gets two
copies, and errors that say `TestServer` does not match `TestServer`.

### 2. Replace the imports

Most of this is a search and replace:

| Replace | With |
|---|---|
| `loco_rs::prelude` | `axa::prelude` |
| `loco_rs::testing::prelude` | `axa_test::prelude` |
| `loco_rs::controller::Routes` | `axa::routes::Routes` |
| `loco_rs::controller::ErrorDetail` | `axa::error::ErrorDetail` |
| `loco_rs::controller::AppRoutes` | `axa::routes::AppRoutes` |
| `loco_rs::app::AppContext` | `axa::AppContext` |
| `loco_rs::environment::Environment` | `axa::Environment` |
| `loco_rs::bgworker::` | `axa::worker::` |
| `loco_rs::task::` | `axa::task::` |
| `loco_rs::model::` | `axa::model::` |
| `loco_rs::schema` | `axa::schema` |
| `loco_rs::db::` | `axa::db::` |
| `loco_rs::` | `axa::` |

`axa::prelude` already includes `IntoResponse`, so remove any separate import the
compiler now reports as unused, including `IntoResponse as _`.

`Environment::Any(name)` becomes `Environment::Other(name)`.

### 3. Turn `Hooks` into plain functions

Change `impl Hooks for App` to `impl App`, drop `#[async_trait]`, and make the
methods `pub`.

Delete `app_name`, `app_version`, and `boot`. Delete `initializers` if it returns
an empty list, and otherwise call each one as a plain function (see step 5).
Check what `seed` did before deleting it.

`routes`, `after_routes`, `truncate`, and the rest keep their bodies. They were
documented by the trait, so a crate with `deny(missing_docs)` now needs a doc
comment on each. A function with no `.await` left should stop being `async`.

Then add the part Loco did for you, which is what makes the flow readable:

```rust,ignore
use axa::{
    Environment, Error, Result,
    cli::{Cli, Command, DbCommand, parse_vars},
    config::{Config as ServiceConfig, Defaults, MB},
    server,
    task::Tasks,
};
use migration::{Migrator, MigratorTrait};

/// `reset` and `truncate` destroy data, so they stay away from anything real.
fn refuse_outside_development(environment: &Environment) -> Result<()> {
    if matches!(environment, Environment::Development | Environment::Test) {
        return Ok(());
    }
    Err(Error::string("refused outside development and test"))
}

fn defaults() -> Defaults {
    Defaults {
        database: "my_service",
        port: 5150,
        production_port: Some(8080),
        body_limit: 2 * MB,
        max_connections: 5,
    }
}

impl App {
    pub async fn context(environment: Environment, config: ServiceConfig) -> Result<AppContext> {
        let db = axa::db::connect(&config.database).await?;
        Ok(AppContext::new(environment, db, config))
    }

    pub async fn router(ctx: &AppContext) -> Result<AxumRouter> {
        let router = Self::routes(ctx).into_router(ctx.clone());
        let router = Self::after_routes(router, ctx).await?;
        axa::middleware::apply(router, &ctx.config.http)
    }

    async fn db(ctx: &AppContext, command: DbCommand) -> Result<()> {
        match command {
            DbCommand::Migrate => Migrator::up(&ctx.db, None).await?,
            DbCommand::Down { steps } => Migrator::down(&ctx.db, Some(steps)).await?,
            DbCommand::Reset => {
                refuse_outside_development(&ctx.environment)?;
                Migrator::fresh(&ctx.db).await?;
            }
            DbCommand::Status => Migrator::status(&ctx.db).await?,
            DbCommand::Truncate => {
                refuse_outside_development(&ctx.environment)?;
                Self::truncate(ctx).await?;
            }
        }
        Ok(())
    }

    pub async fn run(cli: Cli) -> Result<()> {
        let environment = cli
            .environment
            .as_deref()
            .map_or_else(Environment::from_env, Environment::parse);
        // Before anything connects, so it works with the database down.
        if matches!(cli.command, Command::Check) {
            return axa::cli::check(&environment, &defaults());
        }
        let mut config = ServiceConfig::from_env(&environment, &defaults())?;
        config.apply_cli(&cli);
        let ctx = Self::context(environment, config).await?;
        axa::logger::init(env!("CARGO_CRATE_NAME"), &ctx.config.logger);
        if ctx.config.database.auto_migrate && !matches!(cli.command, Command::Db { .. }) {
            Migrator::up(&ctx.db, None).await?;
        }
        match cli.command {
            Command::Start { .. } => {
                let router = Self::router(&ctx).await?;
                server::serve(router, &ctx.config.server).await
            }
            Command::Db { command } => Self::db(&ctx, command).await,
            Command::Task { name, params } => {
                let mut tasks = Tasks::default();
                Self::register_tasks(&mut tasks);
                let Some(name) = name else {
                    for task in tasks.list() {
                        println!("{}\t{}", task.name, task.detail);
                    }
                    return Ok(());
                };
                tasks.run(&name, &ctx, &parse_vars(&params)?).await
            }
            Command::Check => Ok(()), // Answered above.
        }
    }
}
```

**`DbCommand::Reset` drops every table.** `Migrator::fresh` does not pass through
any migration's `down`, so a guard you wrote in a `down` does not run, and neither
does anything that refuses to drop protected data. Loco had no guard here either.
`cli::parse` and `Cli::parse` refuse `down`, `reset`, and `truncate` before your `run` starts, unless
the process is in development or test and `AXA_DESTROY_DATA=true` is set, so a stray
`db reset` on a laptop pointed at a shared database cannot run. The sample's
`refuse_outside_development` is a second check at the call. If `reset` has no place
in your service, make it an error.

If you keep the Loco ports, set them in `Defaults`. Otherwise you get axa's
default, 1241, and any proxy or healthcheck pointing at the old port breaks.

Router order differs from Loco on purpose: `after_routes` adds your layers first, and
`middleware::apply` wraps them. Do not keep Loco's order of `into_app` and then
`after_routes`. A layer added after the standard middleware sits outside it, so an auth
guard's `401` has no request id, security headers, or CORS headers, a preflight reaches
the guard, and that layer gets no panic or timeout protection. A layer that reads a
response body would also get it compressed. It compiles and passes any test that only
checks the status, so test a refusal's headers.

If you override `init_logger`, call it before falling back to
`axa::logger::init`. Replace `init_env_filter::<Self>(a, b)` with
`init_env_filter(env!("CARGO_CRATE_NAME"), a, b)`. If you have `on_shutdown`, call
it after `serve` returns.

### 4. Update `main.rs`

Change only the imports and the body of `main`:

```rust,ignore
use my_service::app::App;

#[tokio::main]
async fn main() -> axa::Result<()> {
    App::run(axa::cli::parse(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_DESCRIPTION"),
    ))
    .await
}
```

**Keep everything above the imports.** That is where `#![forbid(unsafe_code)]`,
your lint attributes, and any `#[global_allocator]` live. Our first conversion
replaced the file whole and lost all three. Nothing failed.

### 5. Replace initializers

An initializer's `after_routes` becomes a function:

```rust,ignore
pub fn install(router: Router) -> Router {
    router.layer(axum::Extension(client))
}
```

Call these in order in `App::router`. Keep the order, because each layer wraps
the ones before it.

### 6. Replace configuration

Delete `config/`, and go through each file first, because everything in it must
now live somewhere:

| YAML | Now |
|---|---|
| `server.port`, `binding`, `host` | `Defaults`, then `PORT`, `BINDING`, `HOST` |
| `database.uri` | `DATABASE_URL`. Required in production |
| `database.max_connections` | `Defaults`, then `DB_MAX_CONNECTIONS` |
| `limit_payload.body_limit` | `Defaults::body_limit`, then `BODY_LIMIT_BYTES` |
| `compression`, `cors` | `HTTP_COMPRESSION`, `CORS_ALLOWED_ORIGINS` |
| `workers.mode` | `WORKER_MODE` |
| `logger` | `LOG_LEVEL`, `LOG_FORMAT`, `LOG_FILTER` |
| `settings:` | Read your own environment variables |
| `mailer:` | Read your own environment variables |
| `dangerously_recreate: true` | **See below** |

Update your `Dockerfile` too: remove `COPY ... config`, and set `APP_ENV` in place
of `LOCO_ENV`. A release build with neither set is production.

**Check your settings for typos.** axa refuses a production boot on a setting that
is set and invalid, such as `HTTP_HSTS=tru`. Run `cargo run -- check` with your real variables. It prints what
each setting came to, with secrets hidden, and fails on what a boot would refuse.

**Tests that read `config/`.** A census or documentation test that walks `config/`
breaks once it is gone. Ours failed, which is the good case: a test that loops over
whatever a glob finds passes with nothing to check. Search your tests for `config/`.

Check your production limits. Any `body_limit` other than the 2 MB default has to
move to `Defaults`, or large uploads start failing.

### 7. Update the test boot

Implement `TestApp`, so `request::<App, _, _>` keeps working and test bodies do
not change:

```rust,ignore
impl TestApp for App {
    async fn boot_test() -> Result<BootResult> {
        let environment = Environment::Test;
        let config = ServiceConfig::from_env(&environment, &defaults())?;
        let ctx = Self::context(environment, config).await?;
        Migrator::fresh(&ctx.db).await.map_err(Error::from)?;
        let router = Self::router(&ctx).await?;
        Ok(BootResult {
            app_context: ctx,
            router: Some(router),
        })
    }
}
```

**Match how your `config/test.yaml` reset the database:**

| `test.yaml` | Test boot |
|---|---|
| `dangerously_recreate: true` | `Migrator::fresh` |
| `auto_migrate` and `dangerously_truncate` | `Migrator::up`, then `Self::truncate` |

Getting this wrong leaves rows behind between tests, which shows up as confusing
count mismatches.

`Migrator::fresh` here runs against whatever `DATABASE_URL` the tests see, and it
drops every table, so do not leave that variable pointing at a real database.

Change a `loco_rs::{TestServer, app::AppContext}` import to
`axa::AppContext` and `axa_test::TestServer`.

Tests that read `routes.prefix` or `routes.handlers` keep working, because those
fields stay public, with `prefix` still an `Option<String>`. But `.actions` on a
handler and `.collect()` on `AppRoutes` are gone, and those tests break. Use
`AppRoutes::mounted()`, or `Routes::mounted()` for one group, which give every route
as `(method, full path)`, with the prefix joined on. `Handler::methods()` gives one
handler's methods. A `HEAD` that comes with `GET` is left out, and a handler for
every method shows as `ANY`.

Loco's `seed::<App>` loaded YAML fixtures. Write a small helper instead, and use
`Entity::insert(model).exec(db)`. Calling `.insert(db)` on the `ActiveModel` runs
`before_save`, which may replace the fixed ids your tests look up.
`cleanup_user_model` also goes, so copy any snapshot filters you still need.

### 8. Background workers

The trait is the same. Registration is synchronous and takes `&mut Queue`:

```rust,ignore
pub fn connect_workers(ctx: &AppContext, queue: &mut Queue) -> Result<()> {
    queue.register(BulkJobWorker::build(ctx));
    Ok(())
}
```

Bring the trait into scope with `axa::worker::BackgroundWorker` so `build`
resolves, and connect the queue in `App::context`:

```rust,ignore
let mut ctx = AppContext::new(environment, db, config);
if ctx.config.workers == WorkerMode::BackgroundQueue {
    let mut queue = Queue::connect(&ctx.config.database.uri, QueueOptions::default()).await?;
    if run_jobs {
        Self::connect_workers(&ctx, &mut queue)?;
    }
    ctx.queue_provider = Some(std::sync::Arc::new(queue));
}
```

Set `run_jobs` only for `start --worker` and `start --all`. A process that only
enqueues needs the queue but not the workers. `App::context` also runs for `db`
and `task` commands, so a check inside `connect_workers` that is registered
unconditionally can stop a migration over a mail setting.

Then run it from `Command::Start` with `server::run`, which stops the server and
the job loops together:

```rust,ignore
let router = if worker { None } else { Some(Self::router(&ctx).await?) };
let queue = (worker || all).then(|| ctx.queue_provider.clone()).flatten();
server::run(router, queue, &ctx.config.server).await
```

`start` alone runs only the server, as in Loco. Use `start --worker` or
`start --all` to run jobs.

Jobs queued in `pg_loco_queue` are not moved. Drain that table before you deploy.

### 9. Readiness

`/_readiness` asks the database with the `sea-orm` feature, and the queue when
there is one. Anything else your service needs goes in as a named check, set in
`App::context` before building the routes:

```rust,ignore
ctx.health_checks = axa::health::Checks::new().add("search", move || {
    let client = client.clone();
    async move { client.ping().await }
});
```

**Moving on to sqlx?** `AppContext::db` is then `()`, and readiness checks no
database until you add your pool as a check, using `axa::sqlx::ping`.

The answer is reused for 5 seconds and a failure names the check, not its error, so
an orchestrator polling faster than that sees the same answer.

### 10. Migrations

Files using `loco_rs::schema` need only the import changed. axa's helpers cover
the column types our services used, so you may see a missing `ColType` variant.
They create `created_at` and `updated_at` and pluralise table names the same way.

Your `seaql_migrations` history is unaffected, because SeaORM still runs them.

## What can go wrong silently

Each of these happened in our conversions, and none caused a compile error.

**A security guard reading the old variable name.** Search your whole codebase
for `LOCO_ENV` and `RUST_ENV`, not just your Dockerfile. One of our services
refused its public development signing key only when `LOCO_ENV=production`. The
Dockerfile moved to `APP_ENV`, the check stopped firing, and a production
container with no signing key would have used that public key. Anyone could then
have forged admin tokens. No test noticed. The fix was to read the shared
environment, and to fail closed on any unknown name.

**A check on one environment name.** Search for `Environment::Production` too.
`matches!(ctx.environment, Environment::Production)` is false for any name but
`production` and `prod`, while axa treats every unknown name as production-like.
With `APP_ENV=prod-eu`, a guard written that way turns itself off while the
service otherwise runs as production. Use `ctx.environment.is_production_like()`.

**A router merged after the middleware.** `Router::merge` keeps the fallback of the
router it is given. So `api.merge(other)`, after `into_app`, throws away the fallback
that carries the middleware, and a request for an unknown path skips every layer:
no request id, no security headers, no error format. It still answers `404`, so
only a test of the response's headers will notice. Write `other.merge(api)`.

**A connection pool of one.** A `test.yaml` with `max_connections: 1` and a 500 ms
`connect_timeout` fails with `PoolTimedOut` when a machine is busy, in tests that
have nothing wrong with them. Look at the load before you look at the code.

**A secret that was only ever warned about.** Check every default that production
used to read from YAML. An empty `get_env(default="")` can become an empty signing
secret. Refuse to boot instead.

**Lost attributes in `main.rs`.** See step 4. Compare the file with the old
version when you finish.

**Email timing.** Loco's `Mailer` queues mail, which keeps responses fast whether
or not an account exists. If you send inline, a known address takes measurably
longer, and that tells an attacker which accounts exist. If you queue it instead,
any login link in the email sits in plain text in a job table. We send from a
detached `tokio::spawn`. A crash mid-send then loses that one email, and the user
asks again.

**CORS.** axa sends no CORS headers in production until
`CORS_ALLOWED_ORIGINS` is set, and removes the permissive default. Remove any
`CorsLayer::permissive()` of your own as well. A browser client calling the API
directly from another origin needs its origin listed. A service that read its
origin from a variable of its own in YAML must carry that across, or the setting
stops working with no error. Development and test default to `*`, so a service
that kept CORS closed there must clear `config.http.cors_origins`.

**Production security headers.** If `config/production.yaml` has a
`secure_headers:` block, compare it with what axa sends. Loco sent them only when
that block was present, and it could be a preset plus overrides:

| Header | Loco's `github` preset | axa in production |
|---|---|---|
| `content-security-policy` | A policy for pages | `default-src 'none'; frame-ancestors 'none'`. Set `HTTP_CSP` |
| `strict-transport-security` | 20 years, unless overridden | One year. Set `HSTS_MAX_AGE_SECONDS` |
| `x-frame-options` | `sameorigin` | `DENY` |
| `x-content-type-options` | `nosniff` | `nosniff` |
| `referrer-policy` | Unset | `no-referrer` |
| `x-download-options`, `x-permitted-cross-domain-policies`, `x-xss-protection` | Sent | Not sent |

The last row matters only to old browsers and Flash. A test of these headers is
worth adding before you convert, because nothing else will notice.

**A stale advisory ignore.** A `deny.toml` that ignores an advisory in one of
Loco's dependencies no longer applies once they go. Delete it. Left in, it does
nothing today, and it hides that advisory if the crate ever comes back through
something else.

**The default local database.** Any script or test that relied on the local
`postgres://loco:loco@localhost:5432/...` must set `DATABASE_URL`, because the
default user is now `postgres`. It fails to connect, so you will see this one, but
only where that default was in use.

**A partial commit.** If you delete `config/` with `git rm` while converting several
services, a plain `git commit` includes those staged deletions for services you
have not finished. Commit with explicit paths.

## Behaviour that changes on purpose

| Change | Effect |
|---|---|
| CORS | Off in production unless origins are listed |
| Request timeout | 60 seconds in production. Raise `REQUEST_TIMEOUT_SECONDS`, or give a slow route its own limit with `Routes::add_with_timeout`, or a group with `Routes::timeout` |
| Security headers | Added to every response, in place of any `secure_headers:` block. See above |
| Content security policy | A strict policy in production. A service that serves pages must set `HTTP_CSP` |
| HSTS | On in production |
| `x-powered-by` | Gone |
| Automatic `If-None-Match` comparison | Kept, in the standard stack, and now weak, with lists and `*`, as RFC 9110 has it. A handler sets an `ETag` and need not return its own `304`. Neither Loco nor `axa` generates a tag unless a route adds `conditional::etag` |
| Missing `DATABASE_URL` in production | Refuses to boot |
| Release build with no `APP_ENV` or `LOCO_ENV` | Production, not development, so it refuses to boot with no `DATABASE_URL`, and binds `0.0.0.0`. A debug build is still development. Set `APP_ENV` where you run a release build locally |
| A setting set but invalid | Refuses to boot in production and any unknown environment, naming the variable. Warned about elsewhere |
| Default local database | `postgres://postgres:postgres@localhost:5432/...`, not `loco:loco` |

## Checking your work

Run these on every converted service:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -- --ignored --test-threads=1   # against a real Postgres
```

Compare the counts with your baseline, then search for what is left:

```sh
git grep -n -E 'loco_rs|loco-rs|LOCO_ENV|RUST_ENV|pg_loco_queue|config/.*\.yaml'
```

Run `cargo update` until `loco-rs` is gone from `Cargo.lock`.

Finally, **start the real binary**, because tests do not cover this:

```sh
APP_ENV=production cargo run -- check   # non-zero with no DATABASE_URL
APP_ENV=production cargo run -- start   # refuses to boot with no DATABASE_URL
curl -i localhost:<port>/_health
```

Check that it refuses bad production settings, serves requests, returns the
security headers, and exits cleanly on `SIGTERM`.
