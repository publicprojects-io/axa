# axa for Rocket users

This page is for someone who knows [Rocket](https://rocket.rs) well and is meeting axa
for the first time. It says what axa is in Rocket's terms, where each Rocket idea goes,
and what will surprise you. It is an orientation, not a reference: the
[README](../../README.md) and the [specification](../../spec/index.md) are that.

## What axa is

Rocket is a framework. It owns the server, the router, the request and response types,
and the launch lifecycle, and your code fills in routes, guards, and fairings that it
calls. axa is not a framework. It is a library of building blocks for services built
directly on [Axum](https://docs.rs/axum). Your handlers are ordinary Axum handlers, your
router is an ordinary `axum::Router`, and your `main.rs` calls axa's functions in the
order you write them. axa adds what sits around the handlers: the middleware stack,
configuration from the environment, error responses, health routes, the command line,
background jobs, and graceful shutdown. There are no route macros, no fairings, and no
launch phases. Anything axa gives you can be read, replaced, or left out on its own.

## Concept map

| Rocket | axa or Axum | Note |
|---|---|---|
| `#[get("/path")]`, `routes![...]`, `mount` | `Routes::new().prefix("/api").add("/path", get(handler))`, collected by `AppRoutes` | Plain function calls, no macros. `AppRoutes::mounted` lists every `(method, path)` for a test |
| `<name>` in a route | `{name}` in the path, read with `Path` | `axa::extract::Path` rejects with a problem that quotes nothing |
| Route ranks and `Outcome::Forward` | None | Axum matches exactly one route per path and method. Branch inside the handler |
| `Responder` | Axum's `IntoResponse` | Handlers usually return `axa::Result<Response>`, built with `format::json`, `format::html`, `format::redirect` |
| `FromRequest` request guards | Axum's `FromRequestParts` and `FromRequest` extractors | Same idea. A rejection is a response, not a forward |
| `&State<T>`, `manage` | `State<AppContext>`, and `ctx.shared_store` for other values | `AppContext` is a plain struct. `SharedStore<T>` clones a value out of the store. State is checked at compile time, so there is no sentinel |
| Sentinels | None needed | A missing `State` is a compile error in Axum. A value missing from `shared_store` is a generic `500` |
| Fairings: `on_request`, `on_response` | Tower layers, or `axum::middleware::from_fn` | Add your own layers before `middleware::apply`, so they sit inside the standard stack |
| Fairings: `on_ignite`, `on_liftoff`, `on_shutdown` | Ordinary code in `main`, before `server::serve` and after it returns | Nothing calls you back |
| Catchers (`#[catch(404)]`, `register`) | None | Every error is an RFC 9457 problem. `axa::Error` maps to the status; see [Errors](../../README.md#errors) |
| `Rocket.toml`, `ROCKET_` variables, Figment | Environment variables only, with `Defaults` in code | No config files. `Config::from_env` reads them, and `Config::from_lookup` for tests |
| Profiles (`debug`, `release`, `ROCKET_PROFILE`) | `APP_ENV`: `development`, `test`, `production` | With it unset, a debug build is development and a release build is production, much as Rocket picks its profile. Any unknown name is production-like |
| `figment.extract::<MyConfig>()` for your own settings | Read your own variables | axa has no public reader for a service's own settings today. `axa::config::redact` hides a secret-shaped value if you print one |
| `secret_key` | No global key | `axa-web-session` requires two distinct keys of at least 64 bytes in production |
| Forms, `Form<T>`, `#[derive(FromForm)]` | Axum's `Form`, in `axa::prelude` | The standard stack turns its plain-text rejection into a problem with no detail. Multipart is Axum's `Multipart` |
| Field validators (`len`, `range`, ...) | `Validatable` over the [`validator`](https://docs.rs/validator) crate | A failure is a `400` with the `code` `validation` and each field's errors, not Rocket's `422` |
| `Contextual<T>` | None | Re-render a failed form yourself from `ModelValidationErrors` |
| `limits` per data kind | One limit, `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT` | `middleware::body_limit(route, bytes)` raises it on one route. Over the limit is a `413` problem |
| `TempFile`, `FileName` | None | Use Axum's `Multipart`, and sanitise the file name yourself |
| `FileServer` | None | Use `tower_http::services::ServeDir` |
| `CookieJar`, private cookies | `axum-extra`'s `CookieJar`, `SignedCookieJar`, `PrivateCookieJar` | axa has no cookie helper. For sign-in, use `axa-web-session` |
| Sessions | `axa-web-session`, over `axum_session` | Signed and encrypted in production, `Secure`, `HttpOnly`, `SameSite=Lax`. Nothing is stored until `sign_in`. `csrf_token` and `csrf_verify` for forms |
| `Flash` | None | Keep the message in the session |
| Shield | The security headers in the standard stack | On every response, in every environment. See [below](#where-axa-is-safer-and-where-it-does-less) |
| `rocket_db_pools`, `Connection<Db>` | One database feature: `postgres-sqlx`, `postgres-seaorm`, `postgres-diesel`, or `postgres-toasty` | `axa::db::connect(&config.database)` opens the pool, and `ctx.db` holds it. Postgres only |
| `rocket_dyn_templates` | `axa-tera-templates` (`tera` feature) | Tera only, loaded once, HTML escaped by default, and a `Page` sends its own CSP |
| `rocket_ws`, `EventStream` | Axum's `WebSocketUpgrade` and `Sse` with `KeepAlive` | axa adds `server::Shutdown`, which a long-lived response waits on to end itself |
| `rocket::local` client | `axa-test`, over `axum-test`, as a dev-dependency | Async only. `request::<App, _, _>` boots the full stack with no port |
| Health checks | `/_health/live`, `/_health/ready`, `/_health/startup` | Built in. Rocket has none |
| Shutdown: `grace`, `mercy`, `Shutdown` | `SHUTDOWN_DRAIN_SECONDS`, `SHUTDOWN_TIMEOUT_SECONDS`, `server::Shutdown` | On `SIGTERM` or Ctrl-C. Readiness refuses from the moment shutdown begins |
| Background jobs | `BackgroundWorker`, `perform_later`, and the `queue` feature | A Postgres queue, worker pool, and schedules. Rocket has none |
| CLI | `axa::cli::parse` and `Cli` | `start`, `db`, `task`, and `check`, which a service matches on itself |
| Logging, `log_level` | `logger::init`, `LOG_LEVEL`, `LOG_FORMAT`, `RUST_LOG` | Every log line carries the request id, method, and route template |
| Metrics, tracing export | `axa-prometheus-metrics`, `axa-opentelemetry` | Rocket has neither |
| Auth | `axa-paseto-auth` or `axa-keycloak-auth`, or your own extractor | Rocket leaves this to a guard you write |
| `ip_header`, `client_ip` | `axa::client::ClientAddr` | `X-Forwarded-For` is read only from `TRUSTED_PROXIES` |
| TLS and mTLS listeners | None | axa's server speaks plain HTTP/1. TLS ends at the proxy in front |

## A first service

This is a complete axa service with two routes, based on the README's
[quick start](../../README.md#quick-start). The manifest:

```toml
[dependencies]
axa = "0.11"
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

And `main.rs`:

```rust,ignore
use axa::{
    Environment, Result,
    config::{Config, Defaults},
    extract::Path,
    logger,
    prelude::*,
    server,
};

async fn hello() -> Result<Response> {
    format::json(serde_json::json!({ "hello": "world" }))
}

async fn greet(Path(name): Path<String>) -> Result<Response> {
    format::json(serde_json::json!({ "hello": name }))
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
                .add("/hello/{name}", get(greet)),
        )
        .into_app(ctx)?;

    server::serve(router, &server_config).await
}
```

It listens on port 1241, serves `/api/hello`, `/api/hello/{name}`, and the three health
routes under the standard middleware, and stops cleanly on `SIGTERM` or Ctrl-C.

The same service in Rocket 0.5, with its `json` feature:

```rust,ignore
#[macro_use]
extern crate rocket;

use rocket::serde::json::{Value, json};

#[get("/hello")]
fn hello() -> Value {
    json!({ "hello": "world" })
}

#[get("/hello/<name>")]
fn greet(name: &str) -> Value {
    json!({ "hello": name })
}

#[launch]
fn rocket() -> _ {
    rocket::build().mount("/api", routes![hello, greet])
}
```

The Rocket version is shorter, because `#[launch]` builds the runtime, reads
`Rocket.toml` and `ROCKET_` variables, and runs the server for you. In axa each of those
steps is a line in `main`: read the environment, read the configuration, start logging,
build the context, build the router, serve. When something goes wrong at boot, the line
that did it is in your own file.

## What is different on purpose

**No macros, and nothing that calls you back.** Rocket's route attributes, fairings, and
sentinels let the framework decide what runs and when. axa forbids route macros and
lifecycle hooks ([P-1](../../spec/index.md#3-principles), and
[Must not be added](../../spec/index.md#must-not-be-added)), so that the control flow
is readable from the service's own code. What a fairing did becomes a Tower layer, or a
line in `main`. The one trait axa asks for, `TestApp`, exists only for tests.

**No `Forward`.** Rocket tries routes in rank order until a guard succeeds. Axum answers
one route per path and method, so which handler answers is never a question. A guard
that fails rejects the request.

**Configuration only from the environment.** There is no `Rocket.toml` and no profile
table. A service states its defaults in code with `Defaults`, and variables override
them, so what a deployment runs with is visible in one place. A setting that is set but
invalid, such as `HTTP_HSTS=tru`, stops a production boot and names the variable
([P-3](../../spec/index.md#3-principles)). Elsewhere it warns and keeps the default.
`cargo run -- check` prints the effective settings, with secrets hidden, and fails on
what a boot would refuse.

**Production refuses what is unsafe.** Rocket fails closed on one setting, a missing
`secret_key` outside debug. axa does it more widely: production with a database and no
`DB_URL` refuses to boot, a `DB_URL` that could go in the clear is refused unless
`DB_ALLOW_INSECURE` is set, `*` as a CORS origin is refused, and a bad
`TRUSTED_PROXIES` list is refused. None of these falls back to a development value.

**Errors never send internal text.** Rocket's catchers answer HTML or JSON for a status.
axa answers every error as an RFC 9457 problem, with `application/problem+json` and a
`code`. An internal failure is a generic `500` whose real text is only logged
([P-4](../../spec/index.md#3-principles)), so a failed query cannot put a hostname in a
response. Axum's own refusals, such as an unmatched path or a rejected extractor, are
made problems too, and their text, which can quote the request, is dropped. A database
that cannot serve, such as a pool with no free connection, answers `503` with
`Retry-After`.

**Client addresses come only from named proxies.** Rocket believes `X-Real-IP` from any
caller by default. axa ignores `X-Forwarded-For` unless the peer is in
`TRUSTED_PROXIES`, and then takes the rightmost entry that is not a proxy, so a caller
cannot name its own address.

**Validation is a `400`.** A `Validatable` failure answers `400` with the `code`
`validation` and an `errors` member by field. A broken check or not-null constraint in
the database answers `422`, and a broken unique or foreign key constraint `409`.

## Where axa is safer, and where it does less

These follow the [Rocket comparison](../../spec/compare/rocket/index.md), checked
against axa 0.11.

### Safer, or stricter, by default

| Rocket | axa |
|---|---|
| Shield sends `nosniff`, `X-Frame-Options: SAMEORIGIN`, and a `Permissions-Policy` that blocks only `interest-cohort`, and no CSP | `nosniff`, `x-frame-options: DENY`, `referrer-policy: no-referrer`, `cross-origin-opener-policy` and `cross-origin-resource-policy` of `same-origin`, and a `permissions-policy` that turns off camera, microphone, location, payment, USB, sensors, and ad topics. Production adds HSTS and `default-src 'none'; frame-ancestors 'none'`. A handler's own value wins |
| HSTS turns on when every listener is TLS outside debug | HSTS is on in production by setting, `HTTP_HSTS` and `HSTS_MAX_AGE_SECONDS` |
| `X-Real-IP` believed from anyone | `X-Forwarded-For` only from `TRUSTED_PROXIES` |
| A request id in log spans | `x-request-id` kept or minted, returned on every response, and on every log line |
| Flash is an unsigned cookie a visitor can write | No flash. A message kept in an `axa-web-session` session is signed |
| Templates in three engines, reloaded in debug | One engine, Tera, with escaping enforced, and a failed render shows the visitor nothing |
| A keep-alive setting | Header-read, keep-alive, and send-stall timeouts, a connection cap, and a request timeout of 60 seconds in production |
| No health routes | Three, one per Kubernetes probe, that never ask a third party |

A service that serves pages must set `HTTP_CONTENT_SECURITY_POLICY`, or render through
`axa-tera-templates`, whose `Page` sends a policy that lets a page load its own files.
Otherwise production's strict default blanks the page.

### Does less, today

| Rocket has | axa today | Use meanwhile |
|---|---|---|
| Typed `extract` of your own settings, with the same rules as Rocket's | No public settings reader | Read `std::env::var` yourself, refuse to boot on a bad production value, and never print a secret |
| `Contextual<T>`, to re-render a form with values and errors | None | Build the template context from the submitted form and `ModelValidationErrors` |
| A form extractor with typed validators | Axum's `Form`, and `Validatable` | `validator`'s derive for the rules |
| `TempFile` uploads with caps and a sanitised `FileName` | None | Axum's `Multipart`, a `tempfile`, and `body_limit` on the route |
| Body limits per data kind and per file extension | One limit | `middleware::body_limit(route, bytes)` per route |
| `FileServer` | None | `tower_http::services::ServeDir` |
| Private cookies | None | `axum-extra`'s `PrivateCookieJar`, or `axa-web-session` |
| Flash messages | None | A value in the session, cleared once read |
| HTML error pages for browsers | Problems only, except a failed Tera render, which is HTML | Your own fallback for page routes |
| Template reloading in development | Templates load once | Restart, or `cargo watch` |
| Handlebars and MiniJinja | Tera only | Call the engine yourself |
| A pool closed at shutdown | The pool is dropped | Close it yourself after `serve` returns |
| MySQL, SQLite, MongoDB, Redis pools | Postgres only for the context's database | Open the pool yourself and keep it in `shared_store` |
| TLS and mTLS listeners | None | Terminate TLS at the proxy or load balancer |
| A blocking test client | Async only | `#[tokio::test]` |

## Moving a service across

Rocket does not run on Axum, so a move is a rewrite of the HTTP layer, not a change of
imports. Your domain code, models, and SQL carry over. Do it in this order:

1. **Get a baseline.** Run the whole test suite, against a real database, and write
   down the counts.
2. **Write `main.rs` first**, as above, with your old port in `Defaults::port` if
   proxies point at it. Rocket's default was 8000; axa's is 1241.
3. **Move configuration.** Go through `Rocket.toml` and every `ROCKET_` variable. Rocket's
   own settings become axa's: `port` and `address` become `PORT` and `BINDING`, a
   `databases` URL becomes `DB_URL`, `limits` becomes `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`
   and `body_limit` on the routes that need more, `log_level` becomes `LOG_LEVEL`, and
   `ip_header` becomes `TRUSTED_PROXIES`. Your own settings become variables you read.
   Set `APP_ENV` where you used `ROCKET_PROFILE`, and remember a release build with
   none is production.
4. **Turn the state into `AppContext`.** The database pool goes in `ctx.db`, opened
   with `axa::db::connect`. Other managed values go in `ctx.shared_store`.
5. **Rewrite routes and guards.** Each `#[get]` becomes an `async fn` and a line in a
   `Routes` group. Each `FromRequest` guard becomes an extractor or a layer. Each
   `Responder` becomes `IntoResponse`, and handlers return `axa::Result`. Replace any
   route that relied on `Forward` or ranks with one handler that branches.
6. **Replace fairings.** Request and response fairings become layers, added before
   `middleware::apply`. A layer added after it sits outside the stack, so its refusals
   get no request id or security headers. Ignite and shutdown code goes in `main`.
7. **Replace catchers.** Remove them, and return `Error` variants. Clients that parsed
   Rocket's error bodies now get problems.
8. **Set the deployment.** Point the liveness, readiness, and startup probes at
   `/_health/live`, `/_health/ready`, and `/_health/startup`. On Kubernetes, set
   `SHUTDOWN_DRAIN_SECONDS` to a little more than the cluster takes to remove a pod, and
   keep `terminationGracePeriodSeconds` above it plus `SHUTDOWN_TIMEOUT_SECONDS`. List
   `TRUSTED_PROXIES` and `CORS_ALLOWED_ORIGINS`. Set `HTTP_CONTENT_SECURITY_POLICY` for
   pages.
9. **Move the tests.** Implement `TestApp` for your service, and replace
   `Client::tracked` with `request::<App, _, _>` from `axa_test::prelude`.

Then test what a Rocket service never had to:

- Each error answers as a problem, and a `500` carries no internal text.
- A refusal from your own guard has `x-request-id` and the security headers.
- `APP_ENV=production cargo run -- check` fails with no `DB_URL`, and with a bad setting.
- A forged `X-Forwarded-For` from an untrusted peer changes nothing.
- The pages you serve still load their scripts and styles in production.
- The real binary exits cleanly on `SIGTERM` with a request in flight.

## Further reading

- [README](../../README.md): [Quick start](../../README.md#quick-start),
  [Routes](../../README.md#routes), [Middleware](../../README.md#middleware),
  [Errors](../../README.md#errors), [Configuration](../../README.md#configuration),
  [Health checks](../../README.md#health-checks), [Testing](../../README.md#testing)
- [The specification](../../spec/index.md), and its
  [principles](../../spec/index.md#3-principles)
- [Health routes](../../spec/health/index.md)
- [Rocket compared with axa](../../spec/compare/rocket/index.md)
- [Background jobs and schedules](../background-jobs/index.md)
- [Idempotency keys](../idempotency-keys/index.md)
- [The transactional outbox](../transactional-outbox/index.md)
- [Upgrading to axa 0.11](../upgrading-to-0.11/index.md)
