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
| `cli::main::<App, Migrator>()` | `App::run(Cli::parse())`, written out in your own code |
| `config/*.yaml` | Environment variables, with defaults in code |
| `LOCO_ENV` | `APP_ENV` (`LOCO_ENV` is still read by axa) |
| `Initializer` | A function that takes and returns a `Router` |
| `use loco_rs::prelude::*` | `use axa::prelude::*` |
| `ctx.shared_store`, `AppContext`, `Routes` | The same names, with the same shape |
| `BackgroundWorker`, `Task` | The same traits. `Queue::register` is now synchronous |
| `pg_loco_queue` | `axa_queue` |
| `loco_rs::schema` | `axa::schema` |
| `request::<App, _, _>` | The same, from `axa::testing::prelude` |
| `cargo loco start` | `cargo run -- start` |
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
axa = { git = "https://github.com/publicprojects-io/axa", rev = "<commit>", features = ["sea-orm", "testing"] }
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
| `loco_rs::testing::prelude` | `axa::testing::prelude` |
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
        let router = Self::routes(ctx).into_app(ctx.clone())?;
        Self::after_routes(router, ctx).await
    }

    async fn db(ctx: &AppContext, command: DbCommand) -> Result<()> {
        match command {
            DbCommand::Migrate => Migrator::up(&ctx.db, None).await?,
            DbCommand::Down { steps } => Migrator::down(&ctx.db, Some(steps)).await?,
            DbCommand::Reset => Migrator::fresh(&ctx.db).await?,
            DbCommand::Status => Migrator::status(&ctx.db).await?,
            DbCommand::Truncate => Self::truncate(ctx).await?,
        }
        Ok(())
    }

    pub async fn run(cli: Cli) -> Result<()> {
        let environment = cli
            .environment
            .as_deref()
            .map_or_else(Environment::from_env, Environment::parse);
        let mut config = ServiceConfig::from_env(&environment, &defaults())?;
        if let Command::Start { binding, port, .. } = &cli.command {
            if let Some(binding) = binding {
                config.server.binding.clone_from(binding);
            }
            if let Some(port) = port {
                config.server.port = *port;
            }
        }
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
        }
    }
}
```

If you keep the Loco ports, set them in `Defaults`. Otherwise you get axa's
default, 1241, and any proxy or healthcheck pointing at the old port breaks.

Router order matches Loco: `into_app` applies the standard middleware, and
`after_routes` then layers on top of it.

If you override `init_logger`, call it before falling back to
`axa::logger::init`. Replace `init_env_filter::<Self>(a, b)` with
`init_env_filter(env!("CARGO_CRATE_NAME"), a, b)`. If you have `on_shutdown`, call
it after `serve` returns.

### 4. Update `main.rs`

Change only the imports and the body of `main`:

```rust,ignore
use axa::cli::Cli;
use clap::Parser;
use my_service::app::App;

#[tokio::main]
async fn main() -> axa::Result<()> {
    App::run(Cli::parse()).await
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
of `LOCO_ENV`.

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

Change a `loco_rs::{TestServer, app::AppContext}` import to
`axa::{AppContext, TestServer}`. Tests that read `routes.prefix` or
`routes.handlers` keep working, because those fields stay public, with `prefix`
still an `Option<String>`.

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
    Self::connect_workers(&ctx, &mut queue)?;
    ctx.queue_provider = Some(std::sync::Arc::new(queue));
}
```

Then run it from `Command::Start`, with `tokio::spawn(queue.run(shutdown))`.
`start` alone runs only the server, as in Loco. Use `start --worker` or
`start --all` to run jobs.

Jobs queued in `pg_loco_queue` are not moved. Drain that table before you deploy.

### 9. Migrations

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
directly from another origin needs its origin listed.

**A partial commit.** If you delete `config/` with `git rm` while converting several
services, a plain `git commit` includes those staged deletions for services you
have not finished. Commit with explicit paths.

## Behaviour that changes on purpose

| Change | Effect |
|---|---|
| CORS | Off in production unless origins are listed |
| Request timeout | 60 seconds in production. Raise `REQUEST_TIMEOUT_SECONDS` for slow endpoints |
| Security headers | Added to every response |
| HSTS | On in production |
| `x-powered-by` | Gone |
| Automatic `If-None-Match` comparison | Gone. A handler that sets its own `ETag` must also return its own `304` |
| Missing `DATABASE_URL` in production | Refuses to boot |

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
APP_ENV=production cargo run -- start   # refuses to boot with no DATABASE_URL
curl -i localhost:<port>/_health
```

Check that it refuses bad production settings, serves requests, returns the
security headers, and exits cleanly on `SIGTERM`.
