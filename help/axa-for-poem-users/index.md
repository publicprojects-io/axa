# Axa for Poem users

This page is for someone who knows [Poem](https://github.com/poem-web/poem) well and is
meeting [axa](../../README.md) for the first time. It says what Axa is in Poem's terms,
where each Poem piece goes, and what is different on purpose. The claims about Poem come
from the [comparison page](../../spec/compare/poem/index.md), which was written from
Poem's source at a named commit.

## What Axa is

Poem is a framework of its own: its own `Endpoint` and `Middleware` traits, its own
router, its own server over `hyper`, and macros such as `#[handler]` and `#[OpenApi]`. It
does not use Axum. Axa is not a framework. It is a library on top of
[Axum](https://docs.rs/axum), so your handlers become ordinary Axum handlers, your
middleware becomes Tower layers, and your `main.rs` calls Axa's functions in an order you
can read. Axa adds what sits around the handlers: a standard middleware stack,
configuration from the environment, RFC 9457 error responses, health routes, graceful
shutdown, jobs, and optional crates for databases, auth, sessions, metrics, and OpenAPI.
Nothing calls into your code from behind. If you want to leave a piece out or replace it,
you do, because every piece is a plain function, struct, or layer.

## Concept map

| Poem | Axa or Axum | Note |
|---|---|---|
| `#[handler] fn` | A plain `async fn` | Axum needs no macro. `axa::prelude` re-exports Axum's `get`, `post`, and the rest |
| `Route::new().at(..).nest(..)` | `Routes::new().prefix(..).add(..)`, collected by `AppRoutes` | Both build an ordinary `axum::Router`. You can also use `Router` directly |
| `/users/:id` | `/users/{id}` | Axum's path syntax |
| `web::Path`, `Query`, `Json`, `Form` | Axum's extractors, or `axa::extract::{Json, Path, Query}` | Axa's versions reject with an `axa::Error`, so the caller gets a problem that quotes nothing they sent |
| `FromRequest` | Axum's `FromRequest` and `FromRequestParts` | The same idea |
| `Data<&T>` with `AddData` | `State<AppContext>`, `AppContext::shared_store`, or `Extension` | `AppContext` is a plain struct passed as Axum state |
| `Middleware<E>`, `EndpointExt::with`, `around` | Tower layers and `axum::middleware::from_fn` | Add your own layers first, then `axa::middleware::apply` last. See below |
| `RequestId`, `Tracing`, `CatchPanic`, `Cors`, `Compression` | The standard stack in `axa::middleware::apply` | One stack, in a fixed order, with safe defaults per environment |
| `SizeLimit` | The stack's body limit, `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT` | Counts a streamed body as it arrives. `middleware::body_limit(route, bytes)` raises it on one route |
| `SensitiveHeader` | Already in the stack | `authorization`, `proxy-authorization`, `cookie`, and `set-cookie` are marked sensitive |
| `SetHeader`, `PropagateHeader`, `NormalizePath` | `tower-http` layers | Axa adds nothing here. The stack already sets the security headers |
| `ForceHttps` | None | Axa sends HSTS in production and leaves TLS and redirects to the proxy |
| `poem::Error`, `ResponseError` | `axa::Error`, returned as `axa::Result<T>` | Becomes an RFC 9457 problem. A `500` sends no internal text |
| Configuration | `Config::from_env` with `Defaults` in code | Poem has none of its own. Axa reads only environment variables |
| Database | `axa-postgres-sqlx`, `axa-postgres-seaorm`, `axa-postgres-diesel`, or `axa-postgres-toasty` | Poem has none. One feature makes `AppContext::db` its pool, and readiness checks it |
| Background jobs | `BackgroundWorker`, and the `queue` feature on Postgres | Poem has none. MySQL and SQLite queues are crates of their own |
| `poem-openapi` `ApiKey`, `Basic`, `Bearer` | `axa-paseto-auth` or `axa-keycloak-auth` | Poem's traits extract a credential. Axa's crates verify a bearer token and enforce ABAC. Neither has API keys |
| `CookieSession`, `ServerSession`, Redis sessions | `axa-web-session`, over `axum_session` | Signed and encrypted in production. Nothing stored until `sign_in` |
| `Csrf`, `CsrfVerifier` | `axa_web_session::csrf_token` and `csrf_verify` | Bound to the session, not a stateless cookie pair |
| `RealIp` | `axa::client::ClientAddr`, from `TRUSTED_PROXIES` | Forwarding headers count only from listed proxies |
| Templates | `axa-tera-templates` | Poem's feature list has no template engine |
| `TestClient` | `axa-test`, over `axum-test` | Boots your real router and stack in process |
| `poem-grpc` `HealthReporter` | `/_health/live`, `/_health/ready`, `/_health/startup` | One HTTP route per Kubernetes probe, with named checks |
| `run_with_graceful_shutdown` | `server::serve` or `server::run` | Stops on `SIGTERM` or Ctrl-C. `SHUTDOWN_DRAIN_SECONDS` and `SHUTDOWN_TIMEOUT_SECONDS` |
| `idle_timeout` | `KEEP_ALIVE_TIMEOUT_SECONDS`, `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS`, `SEND_STALL_TIMEOUT_SECONDS`, `MAX_CONNECTIONS` | Set from the environment |
| `Tracing` and a subscriber | `logger::init`, and the stack's trace layer | Log lines carry the request id, method, and route template, never the real path |
| `OpenTelemetryMetrics` | `axa-opentelemetry`'s `http_metrics` | Off until an endpoint is set |
| `PrometheusExporter` | `axa-prometheus-metrics`: `RequestMetrics` and `scrape` | Labels are method, route template, and status only |
| `TokioMetrics` | None | `axa-tracing` has diagnostics that log a blocked or stalled runtime, but no task metrics |
| `poem-openapi` `#[OpenApi]` | `axa-openapi-docs`, with a `utoipa` or hand-written document | A different design. See [OpenAPI](#openapi) |
| `MaybeUndefined<T>` | None | Use `serde_with`'s `double_option`, or your own enum |
| `Multipart`, `Upload` | Axum's `Multipart`, in the prelude | No spooling helper. Raise the route's limit with `body_limit` |
| `SSE`, WebSocket | Axum's own | `server::Shutdown` is an extractor a stream waits on, to end at shutdown |
| `StaticFilesEndpoint`, `EmbeddedFilesEndpoint` | `tower-http`'s `ServeDir`, or `rust-embed` | Axa adds nothing |
| `RouteDomain` | None | Match `Host` in a handler, or with `axum-extra` |
| `i18n` | None | Use `fluent` directly |
| `rustls`, `native-tls`, OpenSSL, ACME | None | TLS belongs to the proxy or platform in front |
| `poem-grpc` | None. Use `tonic` | See [gRPC, MCP, and Lambda](#grpc-mcp-and-lambda) |
| `poem-mcpserver` | None. Use `rmcp` | As above |
| `poem-lambda` | None. Use `lambda_http` | As above |
| `poem-worker` (Cloudflare Workers) | None | Axa assumes a Tokio server process |
| Nothing | Tasks, the CLI, idempotency keys, the outbox, cursor pages, rate limits, Redis cache, `Accepts-version` | Axa's own. See the [README](../../README.md#installing) |

## A first service

This is the same service in both: one JSON route under `/api`, request logging, panic
catching, and a clean stop on Ctrl-C.

In Poem:

```rust,ignore
use std::time::Duration;

use poem::{
    EndpointExt, Route, Server, get, handler,
    listener::TcpListener,
    middleware::{CatchPanic, Tracing},
    web::Json,
};
use serde_json::{Value, json};

#[handler]
fn hello() -> Json<Value> {
    Json(json!({ "hello": "world" }))
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    tracing_subscriber::fmt::init();
    let app = Route::new()
        .nest("/api", Route::new().at("/hello", get(hello)))
        .with(Tracing)
        .with(CatchPanic::new());
    Server::new(TcpListener::bind("0.0.0.0:3000"))
        .run_with_graceful_shutdown(
            app,
            async move {
                let _ = tokio::signal::ctrl_c().await;
            },
            Some(Duration::from_secs(25)),
        )
        .await
}
```

In Axa, with `axa = "0.12"`, `serde_json`, and `tokio` with `macros` and
`rt-multi-thread`:

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
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
        .into_app(ctx)?;

    server::serve(router, &server_config).await
}
```

The Axa version has no handler macro and no `.with`. `into_app` applies the whole
standard stack, and `with_default_routes` mounts the three health routes. It listens on
port 1241, or `PORT`, and stops on `SIGTERM` as well as Ctrl-C, waiting up to
`SHUTDOWN_TIMEOUT_SECONDS` (25 by default) for requests in flight. It also does several
things the Poem version does not: a request id on every response, security headers, a
request timeout and compression in production, a body limit, and problem responses for
an unknown path or a wrong method. Set `APP_ENV=development` while you try it, because a
release build with no `APP_ENV` is production.

To add a layer of your own, such as an auth guard, build the router, add it, then apply
the stack last, so that the guard's refusals still get the request id and headers:

```rust,ignore
let router = routes.into_router(ctx.clone()).layer(from_fn(require_auth));
let router = axa::middleware::apply(router, &ctx.config.http)?;
```

This is the reverse of how `.with` reads in Poem, where the last `.with` is outermost. In
Axa the standard stack must be outermost, so it goes on last.

## What is different on purpose

These come from the principles in [section 3 of the spec](../../spec/index.md#3-principles).

**No macros and no traits to implement (P-1).** Axa has no `#[handler]`, no route macro,
and no code generation, and the spec forbids adding them. The only trait it asks for is
`TestApp`, for tests. You read the control flow in your own `main.rs`.

**Configuration only from the environment, and production fails closed (P-3).** There
are no config files. `Defaults` in code, then environment variables. In production, a
missing `DB_URL` for a service with a database, a `DB_URL` without TLS, or a setting that
is set but invalid, such as `HTTP_HSTS=tru`, stops the boot and names the variable.
`cargo run -- check` with the `axa::cli` parser prints what each setting came to, with
secrets hidden.

**Errors never send internal text (P-4).** Poem sends an error's `Display` text as plain
text by default. Axa sends an RFC 9457 problem. `Error::BadRequest` and
`Error::CustomError` send your text, `Error::Unauthorized` logs yours and sends fixed
text, and anything else is a `500` with no detail, the real error only logged. Axum's own
rejections, and any plain-text error body, are reshaped the same way.

**Client addresses only from named proxies.** Poem's `RealIp` believes `X-Real-IP`,
`Forwarded`, and `X-Forwarded-For` from any caller. Axa believes `X-Forwarded-For` only
from the addresses in `TRUSTED_PROXIES`, reading from the right, and ignores it when that
is unset. Set it to your load balancer's range, or every client looks like the proxy.

**Safe defaults are not options (P-10).** CORS lists origins in production and refuses
`*`. Sessions are signed and encrypted in production, with no plain mode there. A caller's
request id is cleaned before it is reused. There is no switch back to the weaker form.

**The stack has a fixed order.** In Poem you choose each middleware and its order. In Axa
one function applies one stack, so every service answers the same way, and an auditor
reads it once. You can still skip `apply` and build your own, but then you own the
result.

## OpenAPI

This is the biggest difference for a `poem-openapi` user.

In `poem-openapi`, one `#[OpenApi]` impl block builds both the routes and the document,
so they cannot disagree. Its validators, such as `max_length` or `minimum`, run on input
and appear in the schema. It serves six documentation UIs from bundled JavaScript.

Axa cannot work that way, because it forbids route macros. Routes stay in `Routes` and
Axum. The document comes from somewhere else: written by hand as JSON, or generated by
[`utoipa`](https://docs.rs/utoipa) in your own code, which `axa_openapi_docs` re-exports.
Since the two are written apart, they can drift, and `axa-openapi-docs` catches that in a
test instead of preventing it:

```rust,ignore
use axa::{
    health,
    openapi::{dangling_refs, drift, spec_of},
};

#[test]
fn the_document_matches_the_routes() {
    let spec = spec_of::<ApiDoc>(); // your #[derive(utoipa::OpenApi)] struct
    let drift = drift(
        &app_routes().mounted(),
        &spec,
        &[health::LIVE, health::READY, health::STARTUP],
    );
    assert!(drift.is_empty(), "{drift:?}");
    assert!(dangling_refs(&spec).is_empty());
}
```

`drift` compares each mounted `(method, path)` with the document's `paths`, ignoring
parameter names, and reports what is mounted but undocumented and what is documented but
not mounted. `dangling_refs` reports a local `$ref` that points at nothing. That is all
they check. They do not compare request or response schemas, status codes, or
validation rules, so a field renamed in a struct but not in its schema still passes.
Validation is separate too: `axa::validation::Validatable`, over the `validator` crate,
answers `400` with field errors, and nothing ties those rules to the document. The
[comparison page](../../spec/compare/poem/index.md#what-axa-could-add) proposes a
schema-conformance check for tests to close most of that gap. It does not exist yet.

To serve the document, merge `axa::openapi::router(spec, "Title")` into your router
before `apply`. It serves the JSON at `/api-docs/openapi.json` and one Swagger UI page at
`/swagger-ui`, pinned to one exact version with integrity hashes, and with its own
content security policy so the production default does not blank it. Both are public:
with `axa-paseto-auth`, list them with `public_path`, or do not mount them where the API
should not be described publicly. There is no RapiDoc, ReDoc, or Scalar.

## gRPC, MCP, and Lambda

Axa has no crate for any of these, on purpose: each already has a library that works with
Axum and Tower, and Axa would only wrap it.

- **gRPC: [`tonic`](https://docs.rs/tonic).** Run it on its own port beside the Axum
  server, or combine the two in one listener. Axa's stack is shaped for HTTP and JSON
  (problem bodies, timeouts, body limits), so keep it on the Axum side. For
  `grpc.health.v1`, `tonic-health` gives a reporter you set by hand, as Poem's does. Axa
  does not feed it from `/_health/ready`'s checks, though you can call the same checks
  yourself.
- **MCP: the official [`rmcp`](https://docs.rs/rmcp) SDK.** Its streamable HTTP server is
  a Tower service, so it nests in an Axum router under the standard stack. Check the
  `Origin` header on that endpoint, as the MCP specification asks. The comparison found
  no such check in `poem-mcpserver`.
- **Lambda: [`lambda_http`](https://docs.rs/lambda_http).** It runs an Axum `Router`
  directly, so build the router as usual with `into_app` and hand it to `lambda_http`
  instead of `server::serve`. The middleware and errors still apply. Axa's server
  settings, such as shutdown drain and connection limits, do not, because Lambda runs the
  loop, and the health routes have no probe to answer.

## Where Axa is safer, and where it does less

Poem gives you a wide set of parts and leaves the defaults to you. Axa picks the
defaults. Some of Poem's defaults are ones Axa deliberately does not copy:

| Poem | Axa |
|---|---|
| Error `Display` text in the response | Only logged |
| `RealIp` trusts forwarding headers from anyone | `TRUSTED_PROXIES` only |
| `Csrf::default` uses an all-zero AES key | Session-bound tokens in `axa-web-session`, no key to forget |
| `CookieConfig::default` is a plain cookie | Signed and encrypted required in production |
| `RequestId` with `ReuseId::Use` takes the caller's header as it is | Cleaned first |
| `SizeLimit` reads `Content-Length`, answers `411` without it, and does not count a stream | Counts bytes as they arrive, `413` |
| OTel metrics labelled with `url.full` and the error message | Route template only, so no query strings or error text in metrics |
| No security headers built in | `nosniff`, frame, referrer, opener and resource policy, permissions policy, CSP and HSTS in production |

And things Poem has that Axa does not have today:

| Poem | Use in the meantime |
|---|---|
| Routes and document from one macro | `utoipa` and the `drift` test above |
| Schema validators enforced and documented together | `validator` with `Validatable`, kept in step with the document by hand |
| `MaybeUndefined<T>` for `PATCH` | `serde_with`'s `double_option` |
| `Upload` and spooling a file to disk | Axum's `Multipart` and `tempfile`, with `body_limit` on the route |
| `TokioMetrics` | `tokio-metrics` directly, and `axa-tracing`'s stall diagnostics |
| `ForceHttps`, TLS listeners, ACME, certificate reload | The proxy or platform in front |
| Static and embedded files | `tower-http`'s `ServeDir`, `rust-embed` |
| `i18n` with `Accept-Language` | `fluent` directly |
| `RouteDomain` | A `Host` match in a handler, or `axum-extra` |
| API key and Basic schemes | Your own extractor. Store a hash and compare in constant time |
| `poem-grpc`, `poem-mcpserver`, `poem-lambda`, `poem-worker` | `tonic`, `rmcp`, `lambda_http`. Nothing for Cloudflare Workers |

## Moving a service across

There is no mechanical path, because the router, middleware, and handler traits all
change. Move it in this order, and keep the Poem service running until the new one
passes the same tests.

1. **Handlers.** Remove `#[handler]`, change `:id` to `{id}`, and change extractors to
   Axum's or `axa::extract`'s. Change `Data<&T>` to `State<AppContext>`, keeping your
   own clients in `ctx.shared_store` or an `Extension`.
2. **Errors.** Map your `ResponseError` types to `axa::Error`. Decide for each which text
   the caller may see: only `BadRequest` and `CustomError` send yours.
3. **Routes.** Turn each `Route` tree into `Routes` groups in one `app_routes()`
   function, so a test can call `mounted()` on it.
4. **Middleware.** Drop the layers the standard stack already has: request id, tracing,
   panic catching, CORS, compression, size limit, sensitive headers. Turn the rest into
   Tower layers or `from_fn`, added before `middleware::apply`.
5. **OpenAPI.** If you used `poem-openapi`, write the document with `utoipa`, serve it
   with `axa::openapi::router`, and add the `drift` test before you delete the old code.
6. **`main.rs`.** Configuration from `Config::from_env`, then `logger::init`, the
   context, the router, and `server::serve`, or `server::run` with a job queue. Add
   `axa::cli::parse` if you want `start`, `db`, `task`, and `check`.
7. **Tests.** Implement `TestApp` and send requests through `axa_test::request`, which
   boots the real stack.

Then set, per environment:

| Setting | Why |
|---|---|
| `APP_ENV` | A release build without it is production |
| `PORT` | The default is 1241, not Poem's example 3000 |
| `DB_URL` | Required in production with a database, with `sslmode` set |
| `TRUSTED_PROXIES` | Without it, every client address is the proxy's |
| `CORS_ALLOWED_ORIGINS` | No CORS in production until set |
| `HTTP_CONTENT_SECURITY_POLICY` | The production default blanks a page that loads scripts or styles |
| `REQUEST_TIMEOUT_SECONDS` | 60 seconds in production. Give a slow route its own with `Routes::add_with_timeout` |
| `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT` | Or `Defaults::body_limit`, if uploads are larger |
| `SHUTDOWN_DRAIN_SECONDS` | On Kubernetes, a little more than the cluster takes to remove a pod |

Point the liveness probe at `/_health/live`, readiness at `/_health/ready`, and, for a
slow boot, startup at `/_health/startup`. Add a named check with
`axa::health::Checks::new().add(...)` only for what the service owns, never a third
party.

What to test, because it compiles either way:

- A refusal from your own layer carries `x-request-id` and the security headers. If not,
  the layer was added after `apply`.
- An error that used to send its text now sends a problem, and a `500` sends no detail.
  Update any client that parsed Poem's plain-text bodies.
- A client behind the proxy gets its own address, not the proxy's.
- A large upload still fits its route's limit, and a chunked one is counted.
- `APP_ENV=production cargo run -- check` passes with the real settings, and fails
  without `DB_URL` if the service has a database.

## Further reading

- [README](../../README.md): [middleware](../../README.md#middleware),
  [errors](../../README.md#errors), [configuration](../../README.md#configuration),
  [health checks](../../README.md#health-checks), [testing](../../README.md#testing)
- [The specification](../../spec/index.md), and its [principles](../../spec/index.md#3-principles)
- [Health routes](../../spec/health/index.md)
- [axa and Poem compared](../../spec/compare/poem/index.md)
- [Upgrading to Axa 0.11](../upgrading-to-0.11/index.md), for the current setting names
- [Background jobs and schedules](../background-jobs/index.md)
- [Idempotency keys](../idempotency-keys/index.md)
- [The transactional outbox](../transactional-outbox/index.md)
- [Converting a project from Loco to Axa](../from-loco-into-axa/index.md), for a worked
  migration of a whole service
