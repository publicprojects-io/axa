# Topcoat

Topcoat is a Rust framework for full-stack, server-rendered web apps: HTML templates
in a `view!` macro, async components, routes taken from the module tree, a small
browser runtime that runs Rust expressions as JavaScript, assets, fonts, icons,
cookies, sessions, mail, and localisation. Its repository is
[github.com/tokio-rs/topcoat](https://github.com/tokio-rs/topcoat). I read the `main`
branch at commit
[`c249820`](https://github.com/tokio-rs/topcoat/tree/c249820e2a701dd13556602f97a8dd8e4a8a0ae6),
the `v0.12.0` release of 9 November 2026. The repository was made in April 2026, has
about 6,600 stars and 39 contributors, and one author wrote about 960 of the commits. It
is MIT-licensed. Its crates hold about 123,000 lines of Rust in some 37 crates, many of
them macro and grammar crates. The README says "Early-stage and experimental. Expect
breaking changes", and releases ship every two to three weeks.

Topcoat is not built on Axum. It has its own router on `matchit`, `http`, and `hyper`,
and its own request context `Cx` in place of extractors and state. It reaches Tower
only through an optional `tower` feature, which can mount a Tower service (such as an
Axum router) as a route, run Tower middleware as a Topcoat layer, or expose a Topcoat
router as a Tower service inside an Axum app
([`docs/router/tower.md`][tower-doc], [`crates/topcoat-router/src/tower.rs`][tower]).
Compression uses `tower-http` inside. So this study looks for framework-level ideas
that axa could take as plain functions and layers, not for Axum patterns.

Links below are to files at that commit, under
`https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/`.

[readme]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/README.md
[cargo]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/Cargo.toml
[router-cargo]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/Cargo.toml
[router-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/router.md
[tower-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/router/tower.md
[tower]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/tower.rs
[origin]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/origin.rs
[proxy]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/proxy.rs
[slash]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/trailing_slash.rs
[service]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/service.rs
[listener]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/listener.rs
[router]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/router.rs
[compression]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/compression.rs
[body-limit]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/body_limit.rs
[error-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/router/error.md
[error-dir]: https://github.com/tokio-rs/topcoat/tree/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/error
[sitemap]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/content/sitemap.rs
[content]: https://github.com/tokio-rs/topcoat/tree/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-router/src/content
[context-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/context.md
[memoize-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/context/memoize.md
[cookie-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/cookie.md
[session-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/session.md
[session]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-session/src/session.rs
[mail-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/mail.md
[mail-transport]: https://github.com/tokio-rs/topcoat/tree/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-mail/src/transport
[l10n-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/l10n.md
[negotiate]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-l10n/src/negotiate.rs
[metadata-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/metadata.md
[asset-doc]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/docs/asset.md
[asset-serve]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-asset/src/serve.rs
[cli]: https://github.com/tokio-rs/topcoat/tree/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat-cli/src
[facade-cargo]: https://github.com/tokio-rs/topcoat/blob/c249820e2a701dd13556602f97a8dd8e4a8a0ae6/crates/topcoat/Cargo.toml

## How it is built

| Area | What Topcoat does |
|---|---|
| Shape | A facade crate `topcoat` re-exports sub-crates behind features. The default set is asset, compression, cookie, font, icon, metadata, router, runtime, serve, session, view, and discover ([`crates/topcoat/Cargo.toml`][facade-cargo]). |
| Routing | Handlers are `async fn(cx: &Cx, body?)` marked `#[page]`, `#[layout]`, `#[layer]`, or `#[route]`. Paths come from the module tree (`module_router!`), from the attribute, or from link-time discovery with `inventory` ([`docs/router.md`][router-doc]). `href!` builds links to a handler by its Rust name. |
| Request state | No extractors or `State`. A typed context `Cx` holds request parts and any value added with `.with(...)` at router, route, or layer level. Route-level values are how per-route settings work: `OriginPolicy`, `Compression`, and `BodyLimit` are all read from `Cx`, so one route can override them ([`docs/context.md`][context-doc]). |
| Middleware | A `#[layer]` is `async fn(cx, body, next)`. Layers match by path prefix, outermost least specific. A layer with no path also wraps unmatched requests, so a 404 passes through it ([`docs/router.md`][router-doc]). |
| Per-request cache | `#[memoize]` caches a function's result for the rest of the request, keyed on its arguments and on the context values it read ([`docs/context/memoize.md`][memoize-doc]). |
| Errors | One error type per status (400, 401, 403, 404, 405, 413, 429, 500, 503, redirect, rewrite) in [`src/error`][error-dir], with `ok_or_not_found()`-style methods on `Option` and `Result`. Any other error is a `500` that hides its text. HTML pages catch errors with an `error_boundary` component ([`docs/router/error.md`][error-doc]). There is no problem-details format. |
| CSRF and cross-origin | `OriginPolicy` runs before any layer. It refuses an unsafe method, or a WebSocket handshake, when `Sec-Fetch-Site` is not `same-origin` or `none`, falls back to comparing `Origin` with `Host`, and lets through requests with neither header (non-browser clients). Origins can be trusted by name, and one route can switch it off, such as a signed webhook ([`origin.rs`][origin]). |
| Client address | `TrustedProxies` trusts CIDR networks or a fixed hop count, and reads `X-Forwarded-For`, RFC 7239 `Forwarded`, or one named header. It walks outward from the peer and stops at the first untrusted address ([`proxy.rs`][proxy]). |
| Trailing slash | A route declares whether its path ends in `/`. The other form gets a `308` redirect keeping the query string by default, or is served, or is a `404` ([`trailing_slash.rs`][slash]). |
| Body limit, compression | 2 MiB default limit applied by buffering extractors, overridable per route ([`body_limit.rs`][body-limit]). Gzip and Brotli after every layer, skipping small, encoded, and range responses, overridable per route ([`compression.rs`][compression]). |
| Content types | JSON, forms, multipart (`multer`), HTML, CSS, JS, Wasm, SSE with keep-alive, WebSocket (`tokio-tungstenite`), and an XML sitemap builder that makes paths absolute from a `BaseUrl` ([`content/`][content], [`sitemap.rs`][sitemap]). |
| Serving | `start` reads `HOST` and `PORT` only. `serve` takes any `Listener`, with TCP and Unix-socket implementations. Graceful shutdown with a 30-second drain by default, set with `RouterService::shutdown_timeout` ([`service.rs`][service], [`listener.rs`][listener]). `Router::handle` turns a `Request` into a `Response` without a server, which is how its Cloudflare Workers example runs ([`router.rs`][router]). |
| Cookies | A per-request jar over the `cookie` crate, with signed and private (encrypted) jars, `__Host-` and `__Secure-` prefixes, default attributes, and typed cookie stores that serialise a struct ([`docs/cookie.md`][cookie-doc]). |
| Sessions | Bring-your-own-storage. Topcoat mints a 32-byte token, sets it in a `__Host-` cookie, and gives the app only its SHA-256 hash to store. `start`, `stop`, `refresh` (sliding expiry), and `rotate` (privilege change) are plain functions ([`session.rs`][session], [`docs/session.md`][session-doc]). |
| Mail | A `Mail` builder, a `Mailer` in `Cx`, and a `Transport` trait with SMTP (`lettre`), `.eml` file, and in-memory transports. The memory one is for tests ([`docs/mail.md`][mail-doc], [`transport/`][mail-transport]). |
| Localisation | `accept_language(cx)` and `negotiate(...)` pick a locale; optional ICU number and date formatting and Fluent messages checked at compile time ([`docs/l10n.md`][l10n-doc], [`negotiate.rs`][negotiate]). |
| HTML extras | Page metadata and Open Graph tags per layout and page ([`docs/metadata.md`][metadata-doc]); content-hashed assets served with `max-age=31536000, immutable` ([`docs/asset.md`][asset-doc], [`serve.rs`][asset-serve]); htmx, Alpine AJAX, Datastar, and Tailwind helpers; a component library copied into the app. |
| CLI | `topcoat new`, `dev` (a dev server that proxies to the app), `fmt` (formats macro bodies), `ui`, `asset`, and `l10n` commands ([`crates/topcoat-cli/src`][cli]). |
| Observability | None that I found. No crate depends on `tracing`, `metrics`, or OpenTelemetry ([`Cargo.toml`][cargo]); the docs show logging with `println!` in a layer. |
| Health, config, database, jobs | No health routes beyond an example `GET /api/health`. No configuration layer beyond `HOST` and `PORT`. No database crate; an example uses Toasty. Background jobs, rate limiting, authentication, validation, and OpenAPI are listed as future work in the roadmap ([`README.md`][readme]). |
| Testing | Unit tests drive `Router::handle` directly, and `MemoryTransport` captures mail. There is no request-level test client crate. |

## Compared with axa

| Area | Topcoat | axa |
|---|---|---|
| Relation to Axum | Own router and context; Tower and Axum only through a bridge | Built on Axum; plain Axum routers and layers |
| Style | Framework: attribute macros, module-derived routes, link-time discovery | Library: plain functions, no hidden behaviour |
| Cross-origin writes | `OriginPolicy` on by default, per-route opt-out | CORS layer and session form tokens (`axa-web-session`); no Fetch Metadata or `Origin` check |
| Client address | CIDR or hop count; `X-Forwarded-For`, `Forwarded`, or one named header | `TRUSTED_PROXIES` CIDR list; `X-Forwarded-For` only |
| Trailing slash | `308` to the declared form, or serve both, or `404` | A `"/"` route is served at the prefix; I did not find a redirect for the other form |
| Per-route overrides | Origin policy, compression, body limit through route context | Timeout and body limit per route |
| Errors | Typed errors per status, generic `500`; HTML-oriented | RFC 9457 problems, constraint violations to 409/422, Axum's own refusals rewritten |
| Configuration | `HOST`, `PORT` | Full environment configuration with production refusals |
| Observability | None | Request id, tracing with route template, Prometheus, OpenTelemetry |
| Health | None | `/_health/live`, `/ready`, `/startup` |
| Shutdown | Drain timeout, 30 s | `SHUTDOWN_TIMEOUT_SECONDS`, 25 s |
| Listeners | TCP and Unix socket; `Router::handle` without a server | TCP |
| Sessions | Token hash only in the app's own store; refresh and rotate | `axum_session` over a database, signed and encrypted, renewed on sign-in |
| Cookies outside sessions | Signed, private, prefixed jars and typed stores | Not offered beyond sessions; `axum-extra` covers it |
| Mail | Builder, SMTP, file, memory transports | None |
| Localisation | `Accept-Language` negotiation, ICU, Fluent | None |
| Templates | `view!` macro, components, client runtime | Tera templates |
| Static assets, sitemap | Hashed immutable assets, directory serving, sitemap builder | None |
| SSE, WebSocket, multipart | Own implementations | Axum's own; axa keeps upgrades working |
| Jobs, outbox, idempotency, rate limits, auth, OpenAPI, pagination | Not present (roadmap) | Present |
| Testing | `Router::handle`, memory mail transport | `axa-test` request helpers |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| Fetch Metadata and `Origin` guard for unsafe methods and WebSocket upgrades | Stops cross-site form posts and WebSocket hijacking for cookie-authenticated services without a form token, and covers routes the session token misses | S | High |
| Mail crate with SMTP, file, and memory transports | Most services send sign-up and reset mail; a memory transport makes it testable, and the job queue can send it out of the request | M | Medium |
| Trailing-slash policy (`308` to the declared form) | `/api/items/` against `/api/items` is a common 404 for clients; a redirect that keeps method and body is safe | S | Medium |
| Hop-count proxy trust and RFC 7239 `Forwarded` | Load balancers with changing addresses cannot be listed by CIDR; some proxies send only `Forwarded` | S | Low |
| `Accept-Language` negotiation helper | Lets an API or Tera page pick a locale without a localisation framework | S | Low |
| Unix-socket listener | Common behind a local reverse proxy; no port to guard | S | Low |
| Per-request memoisation helper | Lets an auth guard and a handler share one user lookup without threading it through | S | Low |
| Sitemap response type | Small, but only useful for services that serve public HTML | S | Low |
| Hash-only session tokens with `rotate` | Session records stolen from the database cannot be replayed; worth checking whether `axum_session` stores the raw id, which I did not check | M | Low |

**Fetch Metadata and `Origin` guard.** This fits the core `axa::middleware` module as a
layer, `middleware::origin_guard`, placed in the standard stack after CORS. It would let
`GET`, `HEAD`, and `OPTIONS` through unless they upgrade to WebSocket; let through a
request whose `Sec-Fetch-Site` is `same-origin` or `none`, or whose `Origin` is in
`CORS_ALLOWED_ORIGINS` or matches `HOST`; let through a request with neither header,
since bearer-token clients are not exposed to CSRF; and answer anything else with a
`403` problem. It needs no new dependency. To stay a library and not a hidden rule, it
should be a named layer with a config switch (say `HTTP_ORIGIN_GUARD`, on in
production), and a route opts out by being mounted outside it, as a signed webhook
would be. Topcoat's tests in [`origin.rs`][origin] are a good checklist for the spec
rows.

**Mail crate.** A new optional crate, `axa-mail`, with a `Mail` builder (or a thin
re-export of `lettre`'s `Message`), a `Transport` trait, and three implementations:
SMTP over `lettre` with rustls, an `.eml` file transport for development, and a
memory transport that `axa-test` can hand to a test. Configuration would come from
`MAIL_URL` (`smtps://` or `smtp://` with STARTTLS), and production would refuse the
file transport and plain `smtp://` without STARTTLS, in the same way it refuses an
insecure database URL. A task or job could send mail, so the queue crates stay free of
it. The `Mailer` is a value in the app's state, not a global.

**Trailing-slash policy.** A small function in `axa::routes` or `axa::middleware`,
`trailing_slash::redirect`, that answers a request for the other form of a mounted
path with `308` and the query string kept. `AppRoutes::mounted` already lists every
path, so the layer can check against that list rather than redirecting blindly, and
it must not turn a `404` into a redirect loop. It has to sit after the auth layers'
own trailing-slash checks (`PST-5`), so it cannot downgrade a destructive action.

## What axa should not copy

- **The framework shape.** Attribute macros (`#[page]`, `#[route]`, `#[layer]`),
  module-derived routes, link-time discovery with `inventory`, and the `Cx` context in
  place of extractors all hide behaviour that axa keeps in plain Axum code. axa's
  `Routes` and `AppRoutes::mounted` already give the visibility that discovery trades
  away.
- **`#[memoize]` as a macro.** Its caching keyed on which context values were read is
  clever but opaque. If axa adds request memoisation, it should be a plain function over
  request extensions.
- **The view layer.** `view!`, components, the client runtime that translates Rust to
  JavaScript, shards, Tailwind, htmx, Datastar, the UI component library, fonts, and
  icons are an HTML app framework. axa serves APIs and has Tera for the pages it needs.
- **Asset bundling from the binary and a dev proxy server.** They need a build-time
  tool and a CLI that owns the app's lifecycle.
- **Typed per-status error structs and HTML error boundaries.** axa's single `Error`
  with RFC 9457 problems is a better fit for APIs and already hides internal text.
- **Fluent and ICU formatting.** Too large and product-specific for axa; a locale
  negotiation helper is the part worth taking.
- **Its lack of observability, health checks, and configuration.** These are where axa
  is ahead; nothing to take.
