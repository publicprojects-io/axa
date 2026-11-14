# Axa for Actix Web users

This page is for a developer who knows [Actix Web](https://actix.rs) and its companion
crates in `actix-extras`, and is meeting [axa](../../README.md) for the first time. It
says what Axa is in Actix terms, where each thing you use today lives, and what will
surprise you. It is an orientation, not a reference.

## What Axa is

Axa is a library on [Axum](https://docs.rs/axum), not a framework. There is no `App`
builder, no `HttpServer`, no route macros, and nothing that calls your code. Your
handlers become ordinary Axum handlers, your state is ordinary Axum state, and your
`main.rs` calls Axa's functions in an order you can read. Axa adds what goes around
the handlers: a middleware stack, configuration from the environment, error responses,
health routes, graceful shutdown, background jobs, and optional crates for sessions,
authentication, rate limits, metrics, and the rest. Anything Axa does not cover, you
take from Axum, Tower, or `tower-http`, and it fits without an adapter.

The biggest change is not Axa. It is moving from Actix Web to Axum: another router,
another extractor trait, another middleware model, and futures that must be `Send`.

## Concept map

| Actix Web | Axa or Axum | Note |
|---|---|---|
| `HttpServer::new(...).bind(...).run()` | `axa::server::serve(router, &config.server)` | HTTP/1 only, with no TLS of its own. Put TLS and HTTP/2 in a proxy. One multi-threaded Tokio runtime, not a runtime per worker |
| `workers`, `max_connections`, `keep_alive`, `client_request_timeout` | `MAX_CONNECTIONS`, `KEEP_ALIVE_TIMEOUT_SECONDS`, `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS`, `SEND_STALL_TIMEOUT_SECONDS` | Environment variables. There is no worker count |
| `#[get("/x")]`, `web::resource`, `web::scope` | `Routes::new().prefix("/api").add("/x", get(handler))`, gathered by `AppRoutes` | No macros. `Routes` is plain data a test can read |
| `{id}` in a path | `{id}`, the same syntax in Axum 0.8 | `Path<T>` extracts it as in Actix |
| `NormalizePath` | A group's `"/"` route is served at the prefix with no trailing slash | For more, `tower-http`'s `NormalizePathLayer` |
| Guards (`guard::Header`, `guard::Host`, `Any`, `All`) | None | Axum's method routers pick by method. Match a header in the handler or in a `from_fn` layer |
| `FromRequest` | Axum's `FromRequestParts` and `FromRequest` | `axa::extract`'s `Json`, `Path`, and `Query` reject with a problem that quotes nothing the caller sent |
| `web::Json`, `web::Form`, `web::Query`, `web::Path` | `Json`, `Form`, `Query`, `Path` from `axa::prelude` | The prelude re-exports Axum's |
| `JsonConfig::limit`, `PayloadConfig` | `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, and `middleware::body_limit(route, bytes)` on one route | One limit for the whole body, which binds a handler that reads `Body` itself too |
| `web::Data<T>` | `State(ctx): State<AppContext>` | `AppContext` holds the environment, database, configuration, and a typed store. Put your own values in `ctx.shared_store` and read them with the `SharedStore<T>` extractor |
| `HttpResponse`, `Responder` | `impl IntoResponse`, or `Result<Response>` with `format::json`, `format::html`, `format::redirect` | |
| `ResponseError` | Return `axa::Error`, or convert your error into it | Every error is an RFC 9457 problem. Internal text is logged, never sent |
| `ErrorHandlers` | The standard stack | It turns a bare status, an unmatched path, a wrong method, and an extractor's rejection into a problem |
| `.wrap(middleware)`, `from_fn` | `.layer(...)`, `axum::middleware::from_fn` | Add your layers, then call `axa::middleware::apply` last, so they sit inside the stack |
| `Logger` | The standard stack's tracing layer | Logs the route template, never the path or query |
| `Compress` | `HTTP_COMPRESSION` | On in production. Never compresses a response that sets a cookie or is `no-store` |
| `DefaultHeaders` | The standard stack's security headers | `tower-http`'s `SetResponseHeaderLayer` for anything else |
| `Condition` | `Option<Layer>` | Tower implements `Layer` for `Option` |
| `ConnectionInfo::realip_remote_addr` | `axa::client::ClientAddr` | Believes `X-Forwarded-For` only from `TRUSTED_PROXIES` |
| `actix-settings` | `axa::config::Config::from_env`, with `Defaults` in code | No configuration files |
| Database | `postgres-sqlx`, `postgres-seaorm`, `postgres-diesel`, or `postgres-toasty` feature | Actix leaves this to you. With Axa, `AppContext::db` is the pool and readiness checks it |
| Background jobs | `BackgroundWorker` and the `queue` feature | A Postgres queue with workers and schedules. MySQL and SQLite queues are crates of their own |
| Templates | `axa-tera-templates` (`tera` feature) | Escapes by default, shows nothing of a failed render, and sends a page-friendly CSP |
| `actix_web::test`, `actix-test` | `axa-test`, over `axum-test` | Real requests through the full stack, with no port |
| `awc` | None | Use `reqwest` or another client |
| Health | `/_health/live`, `/_health/ready`, `/_health/startup` | Actix has none. See [Health routes](../../spec/health/index.md) |
| `shutdown_timeout`, `shutdown_signal` | `SHUTDOWN_TIMEOUT_SECONDS`, `SHUTDOWN_DRAIN_SECONDS` | `SIGTERM` and Ctrl-C. Requests and jobs share one limit |
| `tracing-actix-web` | The standard stack, `axa-tracing`, `axa-opentelemetry` | A request id is kept if safe or minted, and always returned |
| Metrics | `axa-prometheus-metrics` (`prometheus` feature) | Request durations by route template |

### The companion crates

| Crate | Use with Axa | Note |
|---|---|---|
| `actix-session` | `axa-web-session` (`session` feature), over `axum_session` | A Postgres or memory store, signed and encrypted in production. Nothing is stored for a visitor until `sign_in`, which also gives the session a new id. `csrf_token` and `csrf_verify` for forms. No cookie-only store |
| `actix-identity` | `axa_web_session::sign_in` and `sign_out`, with the user id put in the session | `SessionOptions::lifetime` is an idle limit, and `max_lifetime` limits a long-term session and its cookie. There is no `login_deadline` counted from sign-in |
| `actix-cors` | `CORS_ALLOWED_ORIGINS` in the standard stack | A list of origins, and `*` refused in production. For an origin function or other options, leave the variable empty, clear `config.http.cors_origins` (development and test default to any origin), and add `tower-http`'s `CorsLayer` before `apply` |
| `actix-limitation`, `actix-governor` | `axa-request-rate-limit` (`ratelimit` feature) | `by_client`, `by_key`, and `lock_out_failures`, with `Retry-After` and `RateLimit-*`. Counted in one process, so a limit multiplies with replicas. For a shared limit, use the gateway's |
| `actix-web-httpauth` (Bearer) | `axa-paseto-auth` (`paseto`) or `axa-keycloak-auth` (`keycloak`) | These verify the token and enforce policy, which `actix-web-httpauth` leaves to you. Every path is protected unless listed as public. A `401` carries `WWW-Authenticate: Bearer` |
| `actix-web-httpauth` (Basic, or a fixed token) | None | Write a `from_fn` layer that compares in constant time, such as with `subtle`. `tower-http`'s is deprecated. `axum-extra`'s `TypedHeader` parses `Authorization` |
| `actix-ws` | `axum::extract::ws`, with Axum's `ws` feature | Upgrades pass through the stack. Take `axa::server::Shutdown` in the handler and close the socket when `shutdown.begun()` resolves |
| `actix-files` | `tower-http`'s `ServeDir` and `ServeFile`, with its `fs` feature | Ranges, `ETag`, and precompressed files. A page needs `HTTP_CONTENT_SECURITY_POLICY` set |
| `actix-multipart` | `axum::extract::Multipart`, in the prelude | The body limit caps the whole form. There are no part counts, per-field limits, or temp files: count and spool in your handler |
| `actix-protobuf` | `axum-extra`'s `Protobuf` | |

## A first service

The same service both ways: one JSON route under `/api`.

In Axa, `src/main.rs`:

```rust,ignore
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

async fn item(Path(id): Path<u32>) -> Result<Response> {
    format::json(serde_json::json!({ "id": id }))
}

#[tokio::main]
async fn main() -> Result<()> {
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
        .add_route(
            Routes::new()
                .prefix("/api")
                .add("/hello", get(hello))
                .add("/items/{id}", get(item)),
        )
        .into_app(ctx)?;

    server::serve(router, &server_config).await
}
```

```toml
[dependencies]
axa = "0.13"
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

In Actix Web:

```rust,ignore
use actix_web::{App, HttpServer, Responder, get, middleware::Logger, web};

#[get("/hello")]
async fn hello() -> impl Responder {
    web::Json(serde_json::json!({ "hello": "world" }))
}

#[get("/items/{id}")]
async fn item(id: web::Path<u32>) -> impl Responder {
    web::Json(serde_json::json!({ "id": id.into_inner() }))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    HttpServer::new(|| {
        App::new()
            .wrap(Logger::default())
            .service(web::scope("/api").service(hello).service(item))
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}
```

The Axa one listens on port 1241, or `PORT`, and also serves the three health routes.
Every response has a request id and the security headers, an unknown path is a `404`
problem, and in production it has a 60-second request timeout, compression, and HSTS.
It stops cleanly on `SIGTERM` or Ctrl-C. A release build with no `APP_ENV` runs as
production.

For a real service, parse the command line with `axa::cli::parse`, so that `start`,
`db migrate`, `task`, and `check` work, as in step 3 of
[Converting a project from Loco to Axa](../from-loco-into-axa/index.md#3-turn-hooks-into-plain-functions).

## What is different on purpose

**No macros, no `App` factory.** Axa forbids route macros and code generation, and
lifecycle hooks a service implements (P-1). A route table is a value: a test can list
what is mounted with `AppRoutes::mounted()` without booting anything. Because there is
no factory closure run once per worker, there is no `web::Data` built outside it to
share: one `AppContext` is cloned into the router.

**Handler futures must be `Send`.** Actix runs single-threaded worker runtimes, so an
`Rc` or a `RefCell` held across an `.await` compiles there. Axum runs on a
multi-threaded runtime and refuses it. Use `Arc` and `tokio::sync`, or drop the value
before the `.await`. This is Axum's rule, not Axa's.

**Middleware order is explicit.** Add your own layers first, then call
`axa::middleware::apply`, or use `AppRoutes::into_app`, which applies it for you. A
layer added after `apply` sits outside the stack, so its refusals get no request id,
security headers, or CORS headers, and no panic or timeout protection.

**Configuration comes only from the environment.** There is no TOML file. A service
states its defaults in code, in `Defaults`, and the environment overrides them.
Production refuses to boot on a missing `DB_URL`, a setting that is set but invalid,
or a bad `TRUSTED_PROXIES` list (P-3). Run `check` to see what each setting came to.

**Errors never send internal text.** Actix's default `ResponseError` response writes
the error's `Display` text as the body. In Axa, anything that is not a client's mistake
is a `500` with no detail, and the real error goes to the log (P-4). `Error::BadRequest`
is the one variant whose text you send. A database error does not reach the client: a
broken constraint is a `409` or `422` with fixed text.

**A client's address comes only through named proxies.** `ConnectionInfo` believes
`Forwarded` and `X-Forwarded-For` from anyone, and its documentation warns against
using it for security. Axa reads `X-Forwarded-For` only from peers in `TRUSTED_PROXIES`
and takes the rightmost untrusted entry. With the variable unset, the header is ignored.
It does not read `Forwarded`, `X-Forwarded-Proto`, or `X-Forwarded-Host`.

**Logs carry the route template, not the path.** A path or query can hold a token or a
person's data, so `/api/items/{id}` is logged and `/api/items/42?token=...` is not.

**Strict defaults in production.** CORS sends nothing until origins are listed. The
`content-security-policy` is `default-src 'none'; frame-ancestors 'none'`, which suits
a JSON API and blanks an HTML page, so a service that serves pages sets
`HTTP_CONTENT_SECURITY_POLICY`. A response that sets a cookie is `no-store`.

**One server, one protocol.** Axa's server speaks HTTP/1 so that no step waits with no
limit, and leaves TLS and HTTP/2 to a proxy. There is no `bind_rustls`, `bind_openssl`,
or `bind_uds`.

## Where Axa is safer, and where it does less

Safer, or stricter by default:

- **Client addresses**, as above.
- **Sessions.** `actix-session`'s cookie store keeps state in the cookie, under 4 KB,
  where it cannot be revoked from the server. `axa-web-session` stores server-side,
  encrypted at rest, and stores nothing until someone signs in.
- **Rate-limit answers.** `actix-limitation` answers a bare `429` with no `Retry-After`,
  and a `500` on every request when Redis fails. `axa-request-rate-limit` sends
  `Retry-After` and `RateLimit-*`, hashes its keys, and bounds its memory.
- **Bearer tokens are verified,** not only parsed, and every path is protected unless
  it is listed as public.
- **Request logs** leave out the path and query.
- **Health and shutdown.** Readiness answers `503` from the moment shutdown begins, and
  `SHUTDOWN_DRAIN_SECONDS` keeps accepting while the orchestrator takes the pod out.

Does less today, and what to use meanwhile:

| Gap | Meanwhile |
|---|---|
| Rate limits shared across replicas | Limit at the gateway, or write a layer over `axa-redis-cache` |
| A deadline counted from sign-in, like `actix-identity`'s `login_deadline` | Record the sign-in time in the session and check it in a layer |
| Basic auth or a fixed token for operator routes | A small `from_fn` layer with a constant-time comparison |
| Multipart part counts, field limits, and temp files | Count fields and spool to a `tempfile` in your handler. Never use the client's file name as a path |
| Static files | `tower-http`'s `ServeDir` |
| TLS, HTTP/2, Unix sockets | A proxy in front |
| `Forwarded` and `X-Forwarded-Proto` | `HOST` sets the public URL. Read the header yourself only behind a proxy you trust |
| Continuing an incoming `traceparent` | Not done on purpose: each trace starts at the service's edge. `axa_tracing::propagate` follows a request across services by its id |
| `Origin` checks like `block_on_origin_mismatch` | `csrf_verify` for forms. For JSON, check `Origin` or `Sec-Fetch-Site` in a layer |

## Moving a service across

There is no mechanical path, because the router and extractors change. Do it in this
order, and keep the old service's tests as the baseline.

1. **Write down what it does.** List every route and method, and the headers your
   clients rely on. Run the whole test suite and keep the counts.
2. **Build the skeleton.** `main.rs` as above, with `axa::cli::parse`, and a `TestApp`
   so `axa-test` can boot it. Turn on one database feature and, if you have jobs,
   `queue`.
3. **Move the handlers.** `web::Data` becomes `State<AppContext>` or `SharedStore<T>`,
   `web::Json` becomes `Json`, and `HttpResponse` becomes `impl IntoResponse`. Fix
   `Send` errors as they come. Replace each `ResponseError` with a conversion into
   `axa::Error`, and decide for each one whether its text may reach the client.
4. **Group the routes.** One `Routes` per `web::scope`. Give slow routes their own
   limit with `Routes::add_with_timeout`, and large uploads their own body limit with
   `middleware::body_limit(route, bytes)`.
5. **Move the middleware.** Drop what the stack already does: `Logger`, `Compress`,
   `DefaultHeaders`, CORS, request ids. Map the companion crates as in the table above,
   and add your own layers before `apply`.
6. **Move the configuration** to environment variables, and set them per environment.
7. **Point the probes** at the new routes, and cut over.

Settings to set before the first deploy:

| Setting | Why |
|---|---|
| `APP_ENV` | A release build with none is production |
| `PORT` | The default is 1241, not your old port |
| `DB_URL` | Required in production, with `sslmode` set unless the database is local |
| `TRUSTED_PROXIES` | Your load balancer's addresses, or every client looks like it |
| `CORS_ALLOWED_ORIGINS` | Empty sends no CORS headers in production |
| `HTTP_CONTENT_SECURITY_POLICY` | Only for a service that serves pages |
| `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT` | The default is 2 MB |
| `REQUEST_TIMEOUT_SECONDS` | 60 in production |
| `SHUTDOWN_DRAIN_SECONDS`, `SHUTDOWN_TIMEOUT_SECONDS` | On Kubernetes, the drain a little longer than the cluster takes to remove a pod, and `terminationGracePeriodSeconds` above both |

Health probes: liveness on `/_health/live`, readiness on `/_health/ready`, and, for a
service slow to boot, startup on `/_health/startup`. No other health path is served.

What to test, beyond your own suite:

- A refusal's headers, not only its status: a `401` from your auth layer should carry a
  request id and the security headers. A test that checks only the status passes with
  a layer in the wrong place.
- An error body: a failing database call answers a `500` with no detail.
- A CORS preflight from a listed origin and from one that is not.
- `X-Forwarded-For` from a trusted proxy and from a client that is not one.
- `APP_ENV=production cargo run -- check` with your real settings, and a real
  `SIGTERM` during a slow request.

[Converting a project from Loco to Axa](../from-loco-into-axa/index.md) is written for
Loco, but its sections on the test boot, background workers, readiness, and
[what can go wrong silently](../from-loco-into-axa/index.md#what-can-go-wrong-silently)
apply here too.

## Further reading

- [README](../../README.md): [Quick start](../../README.md#quick-start),
  [Middleware](../../README.md#middleware), [Errors](../../README.md#errors),
  [Configuration](../../README.md#configuration), and
  [Health checks](../../README.md#health-checks)
- [The specification](../../spec/index.md), with the principles in section 3
- [Health routes](../../spec/health/index.md)
- [Actix Web compared with Axa](../../spec/compare/actix-web/index.md)
- [Background jobs and schedules](../background-jobs/index.md)
- [Idempotency keys](../idempotency-keys/index.md)
- [Upgrading to Axa 0.11](../upgrading-to-0.11/index.md)
