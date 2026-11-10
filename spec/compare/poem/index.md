# Poem

Poem is a Rust web framework of its own, not a service built on Axum. Its repository is
[github.com/poem-web/poem](https://github.com/poem-web/poem). I read the `master` branch
at commit [`eddf799`](https://github.com/poem-web/poem/tree/eddf799728955fa183835ebadd5b834b9ad903d3)
of 28 October 2026. It has about 4,460 stars, is licensed MIT or Apache-2.0, and needs
Rust 1.94. The workspace holds `poem` 4.0.0 (the framework), `poem-openapi` 6.0.0 (typed
OpenAPI by derive macros), `poem-grpc` 0.6.0, `poem-mcpserver` 0.4.0, `poem-lambda` 6.0.0,
`poem-worker` (Cloudflare Workers), and about 380 files of examples. It does not use Axum
at all: it has its own `Endpoint` and `Middleware` traits, its own router, and its own
server over `hyper`. It reaches Tower only through a `tower-compat` feature. So this study
asks a different question from the others: which of Poem's built-in capabilities could
`axa` offer to Axum services as plain functions and layers, and which does Axum, Tower, or
`tower-http` already give them.

Links below are to files at that commit, under
`https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/`.

[features]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/Cargo.toml
[server]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/server.rs
[error]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/error.rs
[csrf-mw]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/csrf.rs
[csrf-web]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/web/csrf.rs
[force-https]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/force_https.rs
[normalize]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/normalize_path.rs
[sensitive]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/sensitive_header.rs
[propagate]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/propagate_header.rs
[panic]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/catch_panic.rs
[requestid]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/requestid.rs
[sizelimit]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/size_limit.rs
[tracing]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/tracing_mw.rs
[otel-metrics]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/opentelemetry_metrics.rs
[tokio-metrics]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/middleware/tokio_metrics_mw.rs
[prom]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/endpoint/prometheus_exporter.rs
[static]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/endpoint/static_files.rs
[embed]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/endpoint/embed.rs
[realip]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/web/real_ip.rs
[multipart]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/web/multipart.rs
[sse]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/web/sse/response.rs
[cookie-config]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/session/cookie_config.rs
[session]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/session/session.rs
[redis-session]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/session/redis_storage.rs
[i18n]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/i18n/mod.rs
[domain]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/route/router_domain.rs
[rustls]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/listener/rustls.rs
[acme]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/listener/acme/mod.rs
[test-client]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/test/client.rs
[test-json]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem/src/test/json.rs
[oai-openapi]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-openapi/src/openapi.rs
[oai-error]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-openapi/src/error.rs
[oai-validation]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-openapi/src/validation/mod.rs
[oai-maybe]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-openapi/src/types/maybe_undefined.rs
[oai-auth]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-openapi/src/auth/mod.rs
[oai-upload]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-openapi/src/types/multipart/upload.rs
[grpc-health]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-grpc/src/health.rs
[grpc-lib]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-grpc/src/lib.rs
[mcp-http]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-mcpserver/src/streamable_http.rs
[lambda]: https://github.com/poem-web/poem/blob/eddf799728955fa183835ebadd5b834b9ad903d3/poem-lambda/src/lib.rs

## How it is built

### Framework shape

A handler is a function marked `#[handler]`, and a middleware is a type that implements
`Middleware<E>` and wraps an `Endpoint`. Every capability is a Cargo feature of `poem`
([`poem/Cargo.toml`][features]): `websocket`, `multipart`, `rustls`, `native-tls`,
`openssl-tls`, `sse`, `static-files`, `compression`, `cookie`, `session`, `redis-session`,
`opentelemetry`, `prometheus`, `tempfile`, `csrf`, `test`, `i18n`, `acme`, `embed`, `xml`,
`yaml`, `requestid`, and `tower-compat`. Most of these are thin wrappers over a crate that
also works with Axum: `multer`, `tokio-tungstenite`, `async-compression`, `fluent`,
`rust-embed`, `tokio-metrics`, `libcsrf`.

### Server and shutdown

[`server.rs`][server] runs over `hyper-util`. `run_with_graceful_shutdown` takes a signal
future and an optional time limit, after which open connections are cancelled.
`idle_timeout` closes a connection that has been quiet too long, and there are settings
for HTTP/2 stream and header limits. Listeners cover TCP, Unix sockets, a combined
listener, `rustls`, `native-tls`, and OpenSSL. The `rustls` listener takes a stream of
configurations, so certificates can be replaced without a restart ([`rustls.rs`][rustls]),
and the `acme` feature obtains certificates from Let's Encrypt itself ([`acme/mod.rs`][acme]).

### Errors

`poem::Error` carries a status and a source. A type implementing `ResponseError` sets its
own status, and by default its response body is the error's `Display` text, as plain text
([`error.rs`][error], lines 34 to 38 and 448 to 450). There is no RFC 9457 form. Poem's own
extractor errors say what failed, such as `failed to parse parameter`, and
`poem-openapi`'s say the same with the reason ([`poem-openapi/src/error.rs`][oai-error]).

### Middleware set

| Middleware | What it does | Source |
|---|---|---|
| `RequestId` | Mints a UUID per request, puts it on a tracing span and the response. With `ReuseId::Use` it takes the caller's header as it is, with no cleaning | [`requestid.rs`][requestid] |
| `Tracing` | A span per request, with the route pattern recorded | [`tracing_mw.rs`][tracing] |
| `CatchPanic` | Turns a panic into `500 internal server error` | [`catch_panic.rs`][panic] |
| `Cors`, `Compression`, `SetHeader`, `AddData`, `CookieJarManager` | As named | `poem/src/middleware/` |
| `SizeLimit` | Refuses by `Content-Length` alone. A request without one gets `411 Length Required`, and a body is not counted as it streams | [`size_limit.rs`][sizelimit] |
| `NormalizePath` | Trims, merges, or adds trailing slashes before routing | [`normalize_path.rs`][normalize] |
| `ForceHttps` | Redirects a plain-HTTP request to HTTPS with `308`, with a filter and port | [`force_https.rs`][force-https] |
| `SensitiveHeader` | Marks named request and response headers sensitive, so `Debug` hides them and HPACK does not index them | [`sensitive_header.rs`][sensitive] |
| `PropagateHeader` | Copies named request headers onto the response | [`propagate_header.rs`][propagate] |
| `Csrf` | Double-submit CSRF with AES-GCM tokens from `libcsrf`: a cookie and a token, checked by a `CsrfVerifier` extractor | [`middleware/csrf.rs`][csrf-mw], [`web/csrf.rs`][csrf-web] |
| `OpenTelemetryMetrics` | Counts requests and errors and records a duration histogram | [`opentelemetry_metrics.rs`][otel-metrics] |
| `TokioMetrics` | Instruments every request future with a `tokio_metrics::TaskMonitor`, and serves poll, idle, and scheduling delay totals as JSON | [`tokio_metrics_mw.rs`][tokio-metrics] |

There is no security-headers middleware; a service sets them with `SetHeader`.

### Observability

Beyond the middleware above, `PrometheusExporter` serves a Prometheus `Registry` at a
route ([`prometheus_exporter.rs`][prom]), and the `prometheus` feature bridges
OpenTelemetry metrics into it, so the `OpenTelemetryMetrics` histogram reaches a
Prometheus scraper. The metric labels are the method, `url.full`, the route pattern, the
status, and on an error the error's message ([`opentelemetry_metrics.rs`][otel-metrics],
lines 81 and 112).

### Client address

`RealIp` takes the first of `X-Real-IP`, `Forwarded`, and `X-Forwarded-For` that parses,
and falls back to the socket address ([`real_ip.rs`][realip]). It trusts these headers
from any caller; there is no list of trusted proxies.

### Sessions and cookies

`CookieSession` keeps the whole session in a cookie, and `ServerSession` keeps it in memory
or Redis ([`redis_storage.rs`][redis-session]). `CookieConfig::default` uses
`CookieSecurity::Plain`, with `secure` and `http_only` on, and `SameSite` unset
([`cookie_config.rs`][cookie-config]). `Session::renew` gives a session a new id and
`purge` ends it ([`session.rs`][session]).

### Uploads, streaming, static files

`Multipart` streams fields over `multer`, and `Field::tempfile` spools one to a temporary
file ([`multipart.rs`][multipart]). In `poem-openapi`, `Upload` exposes a file's name,
type, and size, and can be read as bytes, a reader, or a file
([`upload.rs`][oai-upload]). `SSE` has an optional keep-alive ([`sse/response.rs`][sse]).
`StaticFilesEndpoint` serves a directory with optional listing, index file, and SPA
fallback ([`static_files.rs`][static]), and `EmbeddedFilesEndpoint` serves files compiled in
with `rust-embed` ([`embed.rs`][embed]).

### Other

`i18n` loads Fluent resources and negotiates a locale from `Accept-Language`
([`i18n/mod.rs`][i18n]). `RouteDomain` routes by `Host`, with wildcards
([`router_domain.rs`][domain]). `TestClient` drives an endpoint in process, with
assertions on status, headers, text, and a JSON walker such as `assert_i64` and
`assert_contains` ([`test/client.rs`][test-client], [`test/json.rs`][test-json]).

### poem-openapi

A service writes an `impl` block marked `#[OpenApi]`, with `#[oai(path, method)]` on each
handler. The macros build both the routes and the OpenAPI document, so they cannot
disagree. `OpenApiService` sets servers, contact, licence, extra headers, a URL prefix,
and webhooks, and serves Swagger UI, RapiDoc, ReDoc, Scalar, OpenAPI Explorer, or
Stoplight Elements from JavaScript bundled into the crate ([`openapi.rs`][oai-openapi]).
Schema validators run on input and appear in the document: `maximum`, `minimum`,
`multiple_of`, `max_length`, `min_length`, `pattern`, `max_items`, `min_items`,
`unique_items`, `max_properties`, `min_properties` ([`validation/mod.rs`][oai-validation]).
`MaybeUndefined<T>` tells an absent field from a `null`, for `PATCH`
([`maybe_undefined.rs`][oai-maybe]). `ApiKey`, `Basic`, and `Bearer` security schemes are
traits that only take the credential out of the request; checking it is the service's
own code ([`auth/mod.rs`][oai-auth]). I did not read the derive crate's internals.

### poem-grpc, MCP, Lambda

`poem-grpc` is its own gRPC server and client, with code generation, compression,
reflection, and the standard `grpc.health.v1` service. Its `HealthReporter` sets each
service to serving or not serving by hand, and is not tied to any other check
([`health.rs`][grpc-health], [`lib.rs`][grpc-lib]). `poem-mcpserver` serves the Model
Context Protocol over stdio and streamable HTTP, keeping sessions in a process-local map
with a five-minute idle timeout. I found no check of the `Origin` header in
[`streamable_http.rs`][mcp-http]. `poem-lambda` runs an endpoint under `lambda_http`, and
gives the Lambda context as an extractor ([`lambda/src/lib.rs`][lambda]).

## Compared with axa

| Area | Poem | axa |
|---|---|---|
| Shape | A framework: own traits, router, server, macros | A library over Axum: plain functions and layers, no macros |
| Errors | `Display` text sent as plain text by default | RFC 9457 problems; internal text only logged; extractor rejections reshaped and their quoted values dropped |
| Request id | Fresh UUID, or the caller's header as it is | Caller's id kept after cleaning, or a UUID |
| Body limit | By `Content-Length`; `411` without one | `tower-http` limit that counts a streamed body; `413` |
| Panics, CORS, compression, tracing | Yes | Yes, in one standard stack |
| Security headers | None built in | `nosniff`, frame, referrer, optional HSTS and CSP |
| Trailing slashes | `NormalizePath` | None. `tower-http` has `NormalizePathLayer` |
| HTTPS redirect | `ForceHttps` | None; HSTS only. TLS is left to the proxy |
| Sensitive headers | `SensitiveHeader` | Not marked. `tower-http` has `SetSensitiveHeadersLayer` |
| CSRF | Stateless double-submit, AES-GCM, all-zero key by default | Session-bound token in `axa-web-session` |
| Sessions | Cookie, memory, Redis; plain cookies by default | `axa-web-session`: signed and encrypted required in production, nothing stored before sign-in |
| Client IP | Trusts forwarding headers from anyone | `client::resolve` trusts only `TRUSTED_PROXIES`, from the right |
| HTTP metrics | OTel counter and histogram, Prometheus bridge | OTel request metric in `axa-opentelemetry`; `axa-prometheus-metrics` has no request metric |
| Runtime metrics | `TokioMetrics` task totals as JSON | None |
| Health | gRPC health set by hand | Three Kubernetes probes, named checks, cached rounds |
| Graceful shutdown | Signal and time limit; idle connection timeout | Signal and `SHUTDOWN_TIMEOUT_SECONDS`; header-read and keep-alive limits; connection cap |
| TLS and ACME | rustls, OpenSSL, native-tls, reloadable certs, ACME | None by design |
| OpenAPI | Document and routes from one macro; validators enforced and documented; six UIs bundled | Document from `utoipa` or by hand; drift check against mounted routes; Swagger UI pinned with SRI |
| Validation | Schema validators in `poem-openapi` | `axa::validation` over `validator`; `400` with field errors |
| PATCH tri-state | `MaybeUndefined<T>` | None |
| API key / Basic auth | Extraction traits only | PASETO and Keycloak with ABAC; no API key |
| Uploads | Streamed fields, spool to temp file | Axum `Multipart` re-exported, no helper |
| SSE, WebSockets | Built in | Axum has both; axa keeps upgrades working (SRV-9) and lets SSE pass the ETag layer |
| Static and embedded files | Built in | None. `tower-http` `ServeDir`; `rust-embed` with Axum |
| i18n | Fluent with `Accept-Language` | None |
| Host routing | `RouteDomain` | None. Axum can match `Host` in a handler or with `axum-extra` |
| Testing | In-process `TestClient`, JSON walker | `axa-test` over `axum-test`, booting the real stack |
| Jobs, outbox, idempotency, cache, rate limits, pagination, config, CLI | None | All present |
| gRPC | Own server and client | None. `tonic` serves alongside Axum |
| MCP | `poem-mcpserver` | None. The official `rmcp` SDK has a Tower service for Axum |
| Lambda | `poem-lambda` | None. `lambda_http` runs an Axum `Router` directly |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| Prometheus request metrics layer | `axa-prometheus-metrics` serves a registry but measures no requests. A scrape-only service has no latency or error rate without OTel | S | High |
| Tokio runtime and task metrics | Scheduling delay and slow polls show a blocked runtime before timeouts do. Poem measures task futures; nothing in axa does | S | Medium |
| Mark credential headers sensitive in the standard stack | `Authorization`, `Cookie`, `Set-Cookie`, and `Proxy-Authorization` would stay out of any `Debug` print and HPACK index. One `tower-http` layer | S | Medium |
| Tri-state PATCH field | `null` and absent mean different things in a `PATCH`. Every service writing partial updates meets it | S | Medium |
| Bounded upload helper | Spool a multipart file to a temporary file with a per-file and total size limit and an allowed media type list, answering `413` or `415` as problems | M | Medium |
| OpenAPI schema conformance in tests | Poem cannot drift because one macro makes both. axa forbids that macro, so check instead that a real response body matches its documented schema | M | Medium |
| gRPC health from readiness checks | A service serving `tonic` beside Axum could answer `grpc.health.v1` from the same named checks as `/_health/ready` | S | Low |
| Trailing-slash policy | Already in `tower-http`; at most one line in the README on where to put it | S | Low |
| `Accept-Language` negotiation for Tera templates | Useful for public sites; a function from header and offered locales to one locale | S | Low |
| API key authentication | Service-to-service callers often use keys. Poem only extracts them; a useful crate would store a hash and compare in constant time | M | Low |

**Prometheus request metrics.** Add `axa_prometheus_metrics::http_metrics(&Registry)`
returning a layer, or a function for `from_fn_with_state`, as `axa-opentelemetry` already
does with `telemetry.http_metrics()`. It would register one histogram,
`http_request_duration_seconds`, with the labels `method`, `status`, and `route`, taking
the route template from Axum's `MatchedPath` and `unmatched` when there is none, never the
real path. It depends only on `prometheus`, which the crate already has. It should match
the OTel crate's metric in names and labels, so a service can move from one to the other.

**Tokio runtime and task metrics.** Add a `runtime` module to `axa-prometheus-metrics`
behind a feature, with `register_runtime(&Registry, Handle)` sampling
`tokio_metrics::RuntimeMonitor` on an interval into gauges and counters: workers, busy
time, global queue depth, and mean poll time. Poem's per-request `TaskMonitor` is a
smaller form of this. Serving them through the existing scrape route, rather than Poem's
separate JSON endpoint, keeps one place to look. It depends on `tokio-metrics`, a new
dependency to raise first (P-6), and some runtime metrics need `tokio_unstable`, which
the docs must say.

**Sensitive headers.** Add `tower_http::sensitive_headers::SetSensitiveHeadersLayer` for
the four credential headers inside the request-id layer in `axa::middleware::apply`. It
costs nothing, needs no setting, and protects against a future `Debug` print of a
request. `tower-http` is already a dependency.

**Tri-state PATCH field.** Add `axa::model::Patch<T>`, an enum of `Absent`, `Null`, and
`Value(T)`, with `serde` support through `#[serde(default)]`, and a `utoipa` schema behind
the `openapi` feature. `serde_with::rust::double_option` already does this with
`Option<Option<T>>`, so the gain is a clear name and one documented way. It depends on
nothing new.

**Bounded upload helper.** Add an `axa::upload` module with
`spool(field, &UploadLimits) -> Result<Spooled>`, where `UploadLimits` names the most bytes
per file, the most files, and the allowed media types, and `Spooled` holds a
`tempfile::NamedTempFile`, the client's file name kept apart from the stored name, and the
size. Over a limit it stops reading and answers `413`; a refused type answers `415`. The
route still needs `middleware::body_limit` raised, and the docs should say so. It would
depend on `tempfile`, and Axum's own `Multipart`.

**OpenAPI schema conformance in tests.** Add to `axa-test` an
`assert_matches_schema(&spec, method, path, status, &body)` that finds the response
schema in the document and checks the body against it. Together with the existing route
drift check, this closes most of what Poem's single macro gives, without a macro. It
would depend on `jsonschema` as a dev-only dependency of `axa-test`, which is raised first
(P-6).

## What axa should not copy

- **The framework itself.** `#[handler]`, `#[OpenApi]`, and `Middleware<E>` are what axa
  forbids (route macros, code generation, traits a service implements). Axum already has
  handlers as plain functions, `from_fn` layers, and Tower.
- **Error text in the response.** Poem sends an error's `Display` text by default. axa
  only logs it (P-4).
- **Trusting forwarding headers.** `RealIp` believes `X-Real-IP` and `X-Forwarded-For`
  from any caller, so a client can choose its own address. axa's `client::resolve` with
  `TRUSTED_PROXIES` is the safer design already.
- **A default CSRF key.** `Csrf::default` sets the AES key to `Default::default()`, all
  zero bytes ([`middleware/csrf.rs`][csrf-mw], line 95), and the documented example uses
  `Csrf::new()` without a key. Anyone who knows that can forge tokens. A stateless CSRF
  layer in axa, if ever wanted, must take its key explicitly and refuse to build without
  one (P-3, P-10).
- **Plain cookie sessions by default.** `CookieConfig::default` is `CookieSecurity::Plain`.
  `axa-web-session` already requires signing and encryption in production.
- **Reusing a caller's request id as it is.** axa cleans it first.
- **A body limit that trusts `Content-Length`.** Poem refuses a chunked body with `411`
  and does not count a streamed body. axa's limit counts bytes as they arrive.
- **The full URL and error message as metric labels.** Poem's OTel metrics label each
  request with `url.full` and failed ones with the error message. That gives unbounded
  label cardinality and puts query strings and error text, which can hold personal data,
  into the metrics store. axa labels by route template only.
- **TLS, ACME, and certificate reload.** These belong to the proxy or the platform in
  front of an axa service, and the ACME client is a large dependency.
- **Bundled documentation UIs.** Poem compiles six UIs' JavaScript into the crate. axa
  serves one pinned Swagger UI with integrity hashes, which is smaller to audit.
- **Its own gRPC, MCP, and Lambda crates.** `tonic`, the official `rmcp` SDK, and
  `lambda_http` already run with Axum and Tower. axa would only be wrapping them. Any MCP
  endpoint should also check `Origin`, which I did not find in Poem's.
- **Static files, embedding, WebSockets, SSE, host routing, compression, CORS.**
  Axum, `tower-http`, `axum-extra`, and `rust-embed` already cover them.
- **`SizeLimit`, `NormalizePath`, `PropagateHeader`, `SetHeader` as axa code.** These are
  `tower-http` layers already. axa adds value only where it chooses a safe default for the
  standard stack.
