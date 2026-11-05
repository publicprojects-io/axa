# axa

Small shared library for services built directly on [Axum](https://docs.rs/axum)
and [sqlx](https://docs.rs/sqlx).

This is a library, not a framework: a service's `main.rs` stays ordinary Axum
and calls into this crate. There are no lifecycle hooks or macros, and
`AppContext` is a plain struct passed as ordinary Axum state.

## What it provides

| Module | Provides |
|---|---|
| `routes` | `Routes` and `AppRoutes`, which build an ordinary Axum `Router` |
| `middleware` | Request ids, tracing, panic catching, security headers, CORS, timeout, compression, body limit |
| `health` | `/_ping`, `/_health`, `/_readiness` |
| `error`, `format` | An error type with generic 500s, and response helpers |
| `config`, `environment` | Configuration from the process environment, with no config files |
| `logger`, `server` | Logging setup and graceful shutdown |
| `cli`, `task` | The shared `start`, `db`, and `task` commands |
| `worker`, `queue` | Background jobs and a Postgres `SKIP LOCKED` queue (`queue` feature) |
| `testing` | Request-level test helpers (`testing` feature) |
| `schema`, `model` | SeaORM migration helpers and errors (transitional `sea-orm` feature) |

## Configuration

Settings come from the environment: `APP_ENV`, `PORT`, `BINDING`, `HOST`,
`DATABASE_URL`, `DB_*`, `LOG_LEVEL`, `LOG_FORMAT`, `LOG_FILTER`,
`BODY_LIMIT_BYTES`, `HTTP_COMPRESSION`, `HTTP_HSTS`,
`REQUEST_TIMEOUT_SECONDS`, `CORS_ALLOWED_ORIGINS`, `WORKER_MODE`.

The default port is 1241 (a=1, x=24, a=1). A service can set its own in
`Defaults`, and `PORT` overrides both.

Production requires `DATABASE_URL` when the service has a database, and
refuses `*` as a CORS origin.

## Relationship to Loco

The behaviours here are the ones the Main X Index services relied on in
[Loco](https://loco.rs) (Apache-2.0). They were written afresh, not copied, so
that this crate can keep its own licence terms.

## License

See [LICENSE.md](LICENSE.md).
