# Axa examples

Each example is a standalone Cargo package that builds against the `axa` in this
repository. Run one from its own directory.

| Example | What it shows | Database |
|---|---|---|
| [basic-service](basic-service/) | Settings, routes, health probes, problems, and tests with `oneshot` | None |
| [todo-api](todo-api/) | A Postgres pool in `AppContext::db`, migrations at boot, plain SQL, and a database test | Postgres, with `postgres-sqlx` |
| [multi-tenant](multi-tenant/) | The tenant from a verified PASETO token, a tenant filter on every query, and Postgres row-level security | Postgres, with `postgres-sqlx` and `paseto` |

## Start here

```sh
cd examples/basic-service
cargo run
curl localhost:1241/api/greet/World
```

Then [todo-api](todo-api/README.md), which needs Postgres:

```sh
podman-compose up -d postgres          # from the repository root
cd examples/todo-api
export DB_URL=postgres://axa:axa@localhost:5432/axa_development
cargo run
```

## The pattern every example follows

`main` stays ordinary Axum. It reads the settings, starts the logger, builds the router,
and serves it:

```rust,ignore
use axa::{Environment, Result, config::{Config, Defaults}, logger, prelude::*, server};

async fn hello() -> Result<Response> {
    format::json(serde_json::json!({ "hello": "world" }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let defaults = Defaults { database: "", ..Defaults::default() };
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

- **Errors.** A handler returns `axa::Result<Response>`. `bad_request("...")` is `400`
  with its text, `not_found()` is `404`, `?` on a database error is `409`, `422`, `503`,
  or `500` as fits, and every one is an RFC 9457 problem that quotes nothing the caller
  sent.
- **Extractors.** `axa::extract::{Json, Path, Query}` answer a bad body, path, or query
  as a problem too, where Axum's own answer is plain text.
- **A database.** With a feature such as `postgres-sqlx`, name the database in
  `Defaults`, open the pool with `axa::db::connect(&config.database)`, and pass it where
  this passes `()`. `/_health/ready` then asks it.
- **Tests.** Build the router with `into_app` and drive it with
  `tower::ServiceExt::oneshot`, with no server running.

## Start a service of your own

Use the [service template](../templates/service-template/README.md), which depends on
`axa` from crates.io, rather than copying an example, which depends on this repository by
path.
