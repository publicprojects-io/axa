# A service with its own store

Axa's database features cover sqlx, SeaORM, Diesel, and Toasty. A service whose data
lives somewhere else, such as `rusqlite`, an embedded key-value store, or a client for
another service, turns on none of them. `AppContext::db` is then `()`, and the service
keeps its store in `ctx.shared_store`. This page shows that arrangement from start to
finish.

## Name no database

An empty `database` in `Defaults` says that Axa holds no database, so production asks
for no `DB_URL`, and `check` prints none:

```rust,ignore
use axa::config::{Defaults, MB};

pub fn defaults() -> Defaults {
    Defaults {
        database: "",
        port: 5150,
        body_limit: 8 * MB,
        // The service's own request limit, in code rather than in a Dockerfile.
        request_timeout_seconds: Some(10),
        ..Defaults::default()
    }
}
```

The `db` command has nothing to act on, so the service answers it with an error that
says where its data is.

## Open the store before the server listens

Open the store, and anything else a request needs, before `server::serve`, and fail the
start if it cannot be opened. A store opened lazily, on the first request, leaves a
broken instance answering every request with an error while `/_health/ready` stays
green, so a load balancer keeps it in rotation.

```rust,ignore
use std::sync::{Arc, Mutex};

pub type SharedStore = Arc<Mutex<MyStore>>;

pub fn prepare(ctx: &mut AppContext) -> axa::Result<()> {
    let store: SharedStore = Arc::new(Mutex::new(MyStore::open(&store_path()?)?));
    ctx.shared_store.insert(store.clone());
    // `/_health/ready` answers 503 while the store cannot answer.
    ctx.health_checks = std::mem::take(&mut ctx.health_checks).add("store", move || {
        let store = store.clone();
        async move {
            let store = store.lock().map_err(|_| "the store's lock is poisoned")?;
            store.ping().map_err(|_| "the store did not answer")
        }
    });
    Ok(())
}
```

Add the checks before the routes are built, as the router holds its own copy of the
context. The name of a check is sent to whoever calls `/_health/ready`, so make it a
label. The error text is only logged.

## A missing store is a 503

A handler takes the store from the shared store, and answers `503` when it is missing,
which means that `prepare` did not run. That is a service that is not ready, which is
not the same as a request that is wrong, so it is not a `404` or a `500`:

```rust,ignore
use axum::http::StatusCode;

fn store(ctx: &AppContext) -> axa::Result<SharedStore> {
    ctx.shared_store.get::<SharedStore>().ok_or_else(|| {
        axa::Error::with_detail(StatusCode::SERVICE_UNAVAILABLE, "the store is not open")
    })
}
```

`Error::with_detail` sends the sentence as the problem's `detail`. The standard stack
drops the text of a handler that answers `(StatusCode, String)`, and logs a warning
naming the route when it does.

## Layers of the service's own

A rate limit or an authentication guard goes inside the standard stack, so that its
`429` or `401` has a request id and the security headers. `AppRoutes::into_app_with`
puts it there:

```rust,ignore
pub fn router(ctx: &AppContext) -> axa::Result<axum::Router> {
    routes().into_app_with(ctx.clone(), |router| Ok(router.layer(rate_limit()?)))
}
```

Have the server and the tests build the router through this one function, so that a
test of a status also tests the layers.

## Tasks open their own store

A task does not run `prepare`, so it finds nothing in `ctx.shared_store`. Have each task
open the store itself, from the same opener the server uses, so the path cannot drift.
An argument naming another file lets a task check a restored backup, which is never at
the path the service has open.

Build the task's configuration with `Config::for_task`. It does not read the settings
only a listening server uses, such as `PORT` or `CORS_ALLOWED_ORIGINS`, so a task an
operator runs while the server is misconfigured is not refused for them:

```rust,ignore
pub async fn run(cli: Cli) -> axa::Result<()> {
    let environment = cli
        .environment
        .as_deref()
        .map_or_else(Environment::from_env, Environment::parse);
    if matches!(cli.command, Command::Check) {
        return axa::cli::check(&environment, &defaults());
    }
    if let Command::Health { ready, port } = cli.command {
        return axa::cli::health(&environment, &defaults(), ready, port).await;
    }
    let mut config = if matches!(cli.command, Command::Task { .. }) {
        Config::for_task(&environment, &defaults())?
    } else {
        Config::from_env(&environment, &defaults())?
    };
    config.apply_cli(&cli);
    axa::logger::init(env!("CARGO_CRATE_NAME"), &config.logger);
    let mut ctx = AppContext::new(environment, (), config);
    match cli.command {
        Command::Start { .. } => {
            prepare(&mut ctx)?;
            axa::server::serve(router(&ctx)?, &ctx.config.server).await
        }
        Command::Task { name, params } => {
            let tasks = tasks();
            let Some(name) = name else {
                for task in tasks.list() {
                    println!("{}\t{}", task.name, task.detail);
                }
                return Ok(());
            };
            tasks.run(&name, &ctx, &axa::cli::parse_vars(&params)?).await
        }
        Command::Db { .. } => Err(axa::Error::string(
            "this service keeps its data in its own store, not in a database of Axa's",
        )),
        Command::Check | Command::Health { .. } => Ok(()),
    }
}
```

## Tests

`AppContext::for_test(())` is a context in the `test` environment that reads nothing
from the process environment. Put a store in it the way `prepare` does, often an
in-memory one, and send requests to the router that `router` builds.

```rust,ignore
#[tokio::test]
async fn a_missing_record_is_a_404() {
    let mut ctx = AppContext::for_test(()).unwrap();
    let store = MyStore::in_memory().unwrap();
    ctx.shared_store.insert::<SharedStore>(Arc::new(Mutex::new(store)));
    let router = router(&ctx).unwrap();
    // ...
}
```
