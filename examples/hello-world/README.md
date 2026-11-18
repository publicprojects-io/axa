# Hello World

The smallest Axa service: one route that answers `Hello, World!`, the three health
probes, and the standard middleware, with no database. The whole service is
[`src/main.rs`](src/main.rs), under 70 lines with its test.

## Run it

```sh
cd examples/hello-world
cargo run
```

Then:

```sh
curl localhost:1241/
curl localhost:1241/_health/ready
curl -i localhost:1241/nope   # a 404 problem, application/problem+json
```

`cargo test` drives the router with `tower::ServiceExt::oneshot`, with no server.

## What it shows

- `Config::from_env` reads the settings, and `Defaults { database: "" }` says there is
  no database, so `AppContext::db` is `()`.
- `AppRoutes::with_default_routes` adds `/_health/live`, `/_health/ready`, and
  `/_health/startup`.
- `into_app` adds the standard middleware: request ids, security headers, body limits,
  timeouts, and RFC 9457 problems for every refusal.
- `server::serve` serves on port 1241 by default and stops cleanly on `SIGTERM` or
  Ctrl-C.

Next, [basic-service](../basic-service/README.md) adds JSON, path parameters, and
errors, and [todo-api](../todo-api/README.md) adds Postgres.
