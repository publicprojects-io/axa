# Service template

A starting point for a new service on [Axa](https://github.com/publicprojects-io/axa): an
ordinary Axum `main.rs`, one route, Axa's health probes and middleware, and CI.

## Start a service from it

Copy this directory out of the Axa repository, then rename it:

```sh
cp -R templates/service-template ../your-service
cd ../your-service
git init
```

Replace `service-template` and `service_template` with your service's name, in
`Cargo.toml`, `src/main.rs` (`SERVICE_NAME`), and the docs. Then delete the
`[workspace]` table at the end of `Cargo.toml` if your service joins a workspace of its
own.

```sh
cargo run
curl localhost:1241/api/v1/status
curl localhost:1241/_health/ready
```

## What is in it

```text
service-template/
├── Cargo.toml              # axa, axum, serde, and tokio; strict lints
├── src/
│   ├── main.rs             # Settings, logger, and server
│   └── routes.rs           # The service's routes
├── docs/
│   ├── api.md              # What the service answers
│   └── architecture.md     # How it is built, and a decision log
├── .github/workflows/ci.yml
├── .pre-commit-config.yaml
├── .devcontainer/          # VS Code dev container
└── podman-compose.yml      # Local Postgres and Redis
```

From Axa, with no code of its own:

- **Health probes** at `/_health/live`, `/_health/ready`, and `/_health/startup`.
  Readiness fails as soon as shutdown begins.
- **The standard middleware**: request ids, security headers, CORS, a cross-site write
  guard, body limits, timeouts, and compression in production.
- **Errors** as RFC 9457 problems that quote nothing the caller sent.
- **Settings** from the environment, with production refusing unsafe ones at boot.
- **Graceful shutdown** on `SIGTERM` or Ctrl-C.

## Add a route

In `src/routes.rs`, add a handler that returns `axa::Result<Response>`, and add it to a
group:

```rust,ignore
AppRoutes::with_default_routes().add_route(
    Routes::new()
        .prefix("/api/v1")
        .add("/status", get(status))
        .add("/widgets/{id}", get(show_widget)),
)
```

## Add a database

Turn on a database feature, such as `postgres-sqlx`, name the database in `src/main.rs`,
and pass the pool where `main` passes `()`:

```toml
axa = { version = "0.12", features = ["postgres-sqlx"] }
```

```rust,ignore
let defaults = Defaults { database: SERVICE_NAME, ..Defaults::default() };
// ...
let pool = axa::db::connect(&config.database).await?;
let ctx = AppContext::new(environment, pool, config);
```

`/_health/ready` then asks the database too. Axa's
[`examples/todo-api`](https://github.com/publicprojects-io/axa/tree/main/examples/todo-api)
shows the rest, with migrations and tests.

Every feature, such as `paseto` or `keycloak` for bearer tokens and `queue` for
background jobs, is listed in Axa's
[README](https://github.com/publicprojects-io/axa#installing).

## Test it

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

The tests drive the router with `tower::ServiceExt::oneshot`, with no server running.
