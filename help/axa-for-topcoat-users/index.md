# axa for Topcoat users

This page is for someone who knows [Topcoat](https://github.com/tokio-rs/topcoat) well
and is meeting [axa](../../README.md) for the first time. It says what axa is in
Topcoat's terms, what maps to what, and where the two differ on purpose.

Start with one honest point: the two aim at different things. Topcoat is a framework
for full-stack, server-rendered apps: `view!` templates, async components, a browser
runtime, assets, fonts, icons, mail, and localisation. axa is the plumbing under a web
service: the middleware stack, configuration, errors, health routes, jobs, and
shutdown. Most axa services are JSON APIs. If your Topcoat app is mostly pages and
client reactivity, axa replaces little of what you use, and you would be moving to
plain Axum with Tera templates. If it is mostly API routes, or you need jobs, health
probes, metrics, and configuration that Topcoat does not have yet, axa covers a lot.

## What axa is

axa is a library on [Axum](https://docs.rs/axum), not a framework. Your handlers are
ordinary Axum handlers, your routes become an ordinary `axum::Router`, and your
`main.rs` calls axa's functions in an order you can read. There are no attribute
macros, no routes taken from the module tree, no link-time discovery, and no request
context like `Cx`. State is Axum state (`AppContext`, a plain struct), and request data
comes from Axum extractors. axa adds the standard middleware stack, configuration from
the environment, RFC 9457 error responses, the three Kubernetes health routes, a
command line, background jobs, and graceful shutdown. Any piece can be read, replaced,
or left out.

What that means coming from Topcoat: you write more of the wiring yourself, and you
can see all of it. Nothing runs that your `main.rs` did not ask for.

## Concept map

| Topcoat | axa or Axum | Note |
|---|---|---|
| `Router::builder()`, `module_router!`, `discover()` | `Routes` and `AppRoutes`, which build an `axum::Router` | Every path is written as a string where the route is added. `AppRoutes::mounted` lists `(method, path)` for a test |
| `#[page]`, `#[route]` | A plain `async fn` added with `get(...)`, `post(...)` | No attribute. A handler returns anything that is `IntoResponse`, usually `axa::Result<Response>` |
| `#[layout]`, `Slot` | Nothing | Use Tera's template inheritance (`{% extends %}`) in [`axa-tera-templates`](../../crates/axa-tera-templates) |
| `#[component]`, `view!` | Nothing in axa | Tera templates, or an HTML crate of your choice. See [Templates](#templates-and-server-rendered-pages) |
| `href!` | Nothing | Paths are strings. `AppRoutes::mounted` lets a test check that a linked path is served |
| `cx: &Cx`, `context::<T>(cx)` | Axum extractors: `State`, `Path`, `Query`, `Json`, `Form` | `axa::extract` has `Json`, `Path`, and `Query` that reject with a problem quoting nothing the caller sent |
| `path_param!`, `#[query_params]` | `Path<T>`, `Query<T>` | Serde types, as in any Axum app |
| `RouterBuilder::with(value)` | `AppContext`, and `ctx.shared_store` for values of your own | `SharedStore<T>` is an extractor that clones a value out of the store |
| `#[layer]` | A Tower layer or `axum::middleware::from_fn` | Add yours before `middleware::apply`, so the standard stack wraps them |
| `#[memoize]` | Nothing | Put a computed value in the request's extensions from a layer, or pass it along |
| `OriginPolicy` | Nothing equivalent | `axa-web-session` has form tokens. See [Where axa does less](#where-axa-does-less) |
| Error per status, `ok_or_not_found()` | `axa::Error`, with `not_found()`, `bad_request(...)`, `unauthorized(...)` | Every error is an RFC 9457 problem in `application/problem+json`. Internal text is logged, never sent |
| `error_boundary` | Nothing | An Axum `Router::fallback` that returns HTML is left alone by the standard stack, as it only rewrites errors with no `content-type` or `text/plain` |
| `TrustedProxies` | `TRUSTED_PROXIES`, and the `axa::client::ClientAddr` extractor | A CIDR list, and `X-Forwarded-For` only |
| `BodyLimit` | `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, and `middleware::body_limit(route, bytes)` for one route | 2 MB by default |
| `Compression` | `HTTP_COMPRESSION` | On in production. Never compresses a response that sets a cookie or is `no-store` |
| `HOST`, `PORT` | `PORT`, `BINDING`, `HOST`, and the rest of [Configuration](../../README.md#configuration) | In axa `HOST` is the public URL and `BINDING` is the listening address |
| Database | One of `postgres-sqlx`, `postgres-seaorm`, `postgres-diesel`, `postgres-toasty`, as `AppContext::db` | Topcoat has no database crate; its example uses Toasty, which axa supports with the `postgres-toasty` feature |
| Background jobs | `BackgroundWorker` and the Postgres job queue (`queue` feature) | On Topcoat's roadmap. See [Background jobs and schedules](../background-jobs/index.md) |
| Auth | `axa-paseto-auth` or `axa-keycloak-auth`, bearer tokens | Topcoat lists authentication as future work |
| Sessions | `axa-web-session` (`session` feature), over `axum_session` | Signed and encrypted in production, nothing stored until `sign_in` |
| Cookies outside sessions | Nothing in axa | `axum-extra`'s cookie jars, including signed and private ones |
| Mail | Nothing | Use [`lettre`](https://docs.rs/lettre) directly, from a job |
| Localisation | Nothing | Read `Accept-Language` yourself, or use a localisation crate |
| Assets, fonts, icons | Nothing | `tower-http`'s `ServeDir`, with your own cache headers |
| SSE, WebSocket, multipart | Axum's own | `server::Shutdown` is an extractor a stream waits on to end at shutdown |
| `Router::handle` in tests | `axa-test`'s `request::<App, _, _>`, or `tower::ServiceExt::oneshot` on the router | `axa-test` boots the full router, middleware and all, with no port opened |
| Health | `/_health/live`, `/_health/ready`, `/_health/startup` | Topcoat has none beyond an example route |
| Shutdown | `SHUTDOWN_TIMEOUT_SECONDS` (25 by default) and `SHUTDOWN_DRAIN_SECONDS` | Readiness answers `503` from the moment shutdown begins |
| Logging, metrics | `axa::logger`, request id and tracing in the stack, `axa-prometheus-metrics`, `axa-opentelemetry` | Topcoat has none built in |
| `topcoat new`, `dev`, `fmt` | `axa::cli`: `start`, `db`, `task`, `check` | No generator and no dev server. `cargo run -- check` prints the effective settings |
| Tower bridge | Not needed | axa is Tower and Axum all the way down |

## A first service

The same small app in each: one HTML page at `/` and one JSON route at `/api/hello`.

In axa, with `features = ["tera"]`:

```toml
[dependencies]
axa = { version = "0.11", features = ["tera"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust,ignore
use axa::{
    Environment, Error, Result,
    config::{Config, Defaults},
    logger,
    prelude::*,
    server,
    tera::{Page, Templates},
};
use serde_json::json;

async fn hello() -> Result<Response> {
    format::json(json!({ "hello": "world" }))
}

async fn home(SharedStore(templates): SharedStore<Templates>) -> Result<Page, axa::tera::Error> {
    templates.html("home.html", &json!({ "name": "World" }))
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

    let templates = Templates::from_raw([(
        "home.html",
        "<!DOCTYPE html><html><body><h1>Hello, {{ name }}!</h1></body></html>",
    )])
    .map_err(|error| Error::string(&error.to_string()))?;

    let server_config = config.server.clone();
    let ctx = AppContext::new(environment, (), config);
    ctx.shared_store.insert(templates);

    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().add("/", get(home)))
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
        .into_app(ctx)?;

    server::serve(router, &server_config).await
}
```

It listens on port 1241, serves the two routes and the three health routes under the
standard middleware, and stops cleanly on `SIGTERM` or Ctrl-C. `{{ name }}` is escaped,
because the template name ends in `.html`. A real service loads its templates with
`Templates::from_glob("templates/**/*.html")`.

The same in Topcoat, with explicit paths:

```rust,ignore
use topcoat::{
    Result,
    context::Cx,
    router::{Router, content::Json, page, route},
    view::{View, view},
};

#[page("/")]
async fn home(cx: &Cx) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html><body><h1>"Hello, " ("World") "!"</h1></body></html>
    })
}

#[route(GET "/api/hello")]
async fn hello(cx: &Cx) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "hello": "world" })))
}

#[tokio::main]
async fn main() {
    topcoat::start(Router::builder().page(home).route(hello).build())
        .await
        .unwrap();
}
```

The Topcoat version is shorter, and its template is checked at compile time. The axa
version says where its configuration, logging, state, and middleware come from, and
it has health routes, timeouts, security headers, and a production configuration
check that the Topcoat version does not.

## Templates and server-rendered pages

[`axa-tera-templates`](../../crates/axa-tera-templates), as `axa::tera` with the `tera`
feature, is the part of axa meant for pages. It is small on purpose:

- `Templates` compiles templates once at boot, from a glob or from `(name, source)`
  pairs. Share it through `ctx.shared_store` or your own state.
- `Templates::html` renders a `Page`, which answers with `PAGE_CSP`: files from the
  page's own origin, and no inline script.
- Templates named `.html`, `.htm`, or `.xml` escape every value unless the template
  says `safe`.
- A failed render is logged, and the visitor gets a generic HTML `500`, because Tera's
  errors quote template source.

There are no components, no reactivity, no `$(...)` expressions, and no shards. Tera
is text with inheritance, includes, and macros. For partial updates, htmx or a similar
library loaded as a file from your own origin works under `PAGE_CSP`; an inline
`<script>` does not.

Production sends `content-security-policy: default-src 'none'` unless
`HTTP_CONTENT_SECURITY_POLICY` says otherwise, which suits a JSON API and blanks a page
that loads a stylesheet. A `Page` sends its own policy, and a handler's own header
wins, so pages rendered with `Templates::html` are fine. An HTML response built any
other way needs its own `content-security-policy`, or the service sets
`HTTP_CONTENT_SECURITY_POLICY`.

## What is different on purpose

These come from the [principles](../../spec/index.md#3-principles).

**No macros that hide control flow (P-1).** Topcoat's `#[page]`, `#[layer]`,
`module_router!`, and `#[memoize]` decide things for you that axa leaves in your code.
The spec rules out route macros, code generation, and lifecycle hooks. The cost is more
lines in `main.rs`. The gain is that an auditor reads the startup path, the routes, and
the layer order from the service itself.

**Layer order is yours, and it matters.** Topcoat runs its `OriginPolicy` before any
layer, and compression after every layer. In axa, `AppRoutes::into_app` applies the
standard stack, or you call `middleware::apply` on any router. Add your own layers,
such as an auth guard, first, and apply the stack last:

```rust,ignore
let router = routes.into_router(ctx.clone()).layer(from_fn(require_auth));
let router = axa::middleware::apply(router, &ctx.config.http)?;
```

A layer added after `apply` sits outside the stack, so its refusals get no request id,
security headers, or CORS headers, and no panic or timeout protection.

**Configuration only from the environment, and production refuses unsafe settings
(P-3).** There are no config files. Defaults live in code in `Defaults`, and the
environment overrides them. In production a setting that is set and invalid, such as
`HTTP_HSTS=tru`, stops the boot, a service with a database must have a `DB_URL` that demands TLS unless the database is on the same machine or `DB_ALLOW_INSECURE` is set,
and CORS refuses `*`. `cargo run -- check` shows what each setting came to.

**Errors never send internal text (P-4).** Any failure other than a client's mistake is
a generic `500` problem, and the real error is only logged. Topcoat also hides a
`500`'s text, so this will feel familiar. The difference is the shape: every axa error
is `application/problem+json` with a `code`, including an unmatched path's `404` and a
wrong method's `405`. That suits an API. For a site, add an HTML `fallback` of your
own, which the stack leaves alone because it has a media type.

**Client addresses are trusted only from named proxies.** With `TRUSTED_PROXIES` unset,
`X-Forwarded-For` is ignored and the client is the peer. Set it to the CIDR ranges of
your load balancers. There is no hop-count mode.

**The safe way is the only way (P-10).** When axa replaces a weaker behaviour it
removes the old one rather than keeping it as an option. Expect fewer switches than
Topcoat offers, such as no per-route compression setting.

## Where axa is safer, and where it does less

### Where axa goes further

- **Health and shutdown.** Three probe routes with fixed meanings, cached checks that a
  public caller cannot use to drive load, and a drain period in which readiness
  refuses before the listener closes.
- **Configuration.** A full set of settings with production refusals, against
  Topcoat's `HOST` and `PORT`.
- **Observability.** A request id on every response, tracing with the route template
  (not the real path, which can hold a token), Prometheus metrics, and OpenTelemetry
  export.
- **Errors for APIs.** RFC 9457 problems, database constraint violations answered as
  `409` or `422` with no database text, and Axum's own refusals rewritten so they quote
  nothing.
- **Service plumbing Topcoat lists as future work.** Background jobs with schedules,
  bearer authentication, request rate limits, OpenAPI documents, idempotency keys, a
  transactional outbox, and cursor pagination.
- **Compression and caching.** A response that sets a cookie is `no-store`, and is never
  compressed, which compression can leak (BREACH).

### Where axa does less

Topcoat has these and axa does not, today:

| Topcoat has | Use in the meantime |
|---|---|
| `OriginPolicy`: unsafe cross-origin browser requests and WebSocket handshakes refused by default, using `Sec-Fetch-Site` and `Origin` | For forms over a session cookie, `axa_web_session::csrf_token` in each form and `csrf_verify` on the post. For WebSockets, check `Origin` in the handler. A layer of your own can check `Sec-Fetch-Site` |
| Mail with SMTP, file, and memory transports | `lettre`, sent from a background job |
| Trailing-slash redirect to the declared form | Nothing. A `"/"` route in a group is served at the prefix with no trailing slash, and the other form is not redirected |
| Hop-count proxy trust and RFC 7239 `Forwarded` | List the proxies' CIDR ranges in `TRUSTED_PROXIES`, and have them send `X-Forwarded-For` |
| Unix-socket listener | `server::serve` binds TCP |
| Cookie jars outside sessions | `axum-extra` |
| `Accept-Language` negotiation, Fluent, ICU | A localisation crate of your choice |
| Hashed, immutable assets, fonts, icons, sitemap | `tower-http`'s `ServeDir`, and your own build step |
| `view!`, components, the client runtime | Tera, plus a script library served from your own origin |

Topcoat's sessions give the app only a hash of the token to store. axa's sessions are
`axum_session`'s, signed and encrypted in production; this page does not claim they
store only a hash.

## Moving a service across

Move the API routes first, and only move pages if you are content with Tera. A
practical order:

1. **Decide what moves.** API routes, jobs, and anything that needs health probes move
   well. Pages built on components, signals, and shards have no axa equivalent.
2. **Set up the service shape.** Write `main.rs` from the [first service](#a-first-service),
   or from the [Loco guide's `App::run`](../from-loco-into-axa/index.md#3-turn-hooks-into-plain-functions)
   if you want the `start`, `db`, `task`, and `check` commands.
3. **Move state.** Each value added with `.with(...)` becomes a field of your own state
   or a value in `ctx.shared_store`. A database pool becomes `AppContext::db` with one
   of the database features.
4. **Move handlers.** `cx: &Cx` becomes extractors. `path_param!` becomes `Path<T>`,
   `#[query_params]` becomes `Query<T>`, and a body parameter stays a body extractor.
   Topcoat errors become `axa::Error`: `ok_or_not_found()` becomes
   `.ok_or(Error::NotFound)`.
5. **Move layers.** Each `#[layer]` becomes `from_fn` or a Tower layer, added before
   `middleware::apply`. Per-route `with(...)` settings become `Routes::add_with_timeout`
   or `middleware::body_limit(route, bytes)`. Decide what replaces `OriginPolicy` before
   you move any cookie-authenticated form.
6. **Set the deployment.** `APP_ENV=production`, `PORT` and `BINDING` (and `HOST` as the
   public URL), `DB_URL` with `sslmode=require` or stricter, `TRUSTED_PROXIES`,
   `CORS_ALLOWED_ORIGINS` if a browser on another origin calls the API, and
   `HTTP_CONTENT_SECURITY_POLICY` if you serve HTML other than `Page`. On Kubernetes,
   point the probes at `/_health/live`, `/_health/ready`, and `/_health/startup`, and set
   `SHUTDOWN_DRAIN_SECONDS`. Run `check` against each environment's settings.
7. **Test.** Implement `TestApp` and use `axa-test`'s `request::<App, _, _>`, so tests go
   through the full stack. Test the headers on a refusal as well as its status, a
   cross-site form post now that `OriginPolicy` is gone, a path with a trailing slash,
   and that each page still loads its styles under the production CSP.

During a move, Topcoat's optional `tower` feature can expose a Topcoat router as a Tower
service, so it can be mounted inside the axa app while routes move one at a time. It
then sits inside axa's stack, so check that the two do not both compress or both set
security headers.

## Further reading

- [README](../../README.md): [Routes](../../README.md#routes),
  [Middleware](../../README.md#middleware), [Errors](../../README.md#errors),
  [Configuration](../../README.md#configuration),
  [Health checks](../../README.md#health-checks), [Testing](../../README.md#testing)
- [The specification](../../spec/index.md), and its
  [principles](../../spec/index.md#3-principles)
- [Health routes](../../spec/health/index.md)
- [Topcoat compared with axa](../../spec/compare/topcoat/index.md)
- [Background jobs and schedules](../background-jobs/index.md)
- [Upgrading to axa 0.11](../upgrading-to-0.11/index.md), for the current setting names
- [Converting a project from Loco to axa](../from-loco-into-axa/index.md), for a
  worked `App::run` and test boot
