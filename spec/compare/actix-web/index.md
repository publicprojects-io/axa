# Actix Web

Actix Web is a Rust web framework with its own HTTP server, router, extractors, and
middleware system. It does not use Axum. Its repository is
<https://github.com/actix/actix-web>, read at commit
[`ae1d51a`](https://github.com/actix/actix-web/tree/ae1d51adc04ae0a8cc06b396329fea67cbd4483d)
(10 November 2026, `actix-web` 4.19.0). It has about 24,900 stars and 326 Rust files
across eleven crates, among them `actix-files`, `actix-multipart`, `actix-test`, and the
`awc` client. Its official companion crates live in
<https://github.com/actix/actix-extras>, read at commit
[`c5c41b7`](https://github.com/actix/actix-extras/tree/c5c41b74d38345e081720ff42163bbd598032e6b)
(9 November 2026, about 890 stars, 102 Rust files): `actix-cors`, `actix-identity`,
`actix-limitation`, `actix-protobuf`, `actix-session`, `actix-settings`,
`actix-web-httpauth`, `actix-ws`, and `tracing-actix-web`. `actix-governor` is a
community crate in its own repository,
<https://github.com/AaronErhardt/actix-governor>, read at `b14d4da`.

> **Since this was written**, axa 0.11 added a `401` carries `WWW-Authenticate: Bearer`; `server::Shutdown` lets a handler wait for shutdown. Those rows below are marked "Done in 0.11", and the rest stand as written.

Because Actix Web is a framework, this page does not ask how it builds an Axum service.
It asks which of its capabilities an Axum service still lacks, which Axum, Tower, or
`tower-http` already give it, and which of the rest `axa` could offer as a library.


## How it is built

### Server

`HttpServer` in [`actix-web/src/server.rs`](https://github.com/actix/actix-web/blob/ae1d51adc04ae0a8cc06b396329fea67cbd4483d/actix-web/src/server.rs) runs a
number of single-threaded worker runtimes and sets, among others, `workers`,
`backlog`, `max_connections` and `max_connection_rate` (per worker),
`client_request_timeout`, `client_disconnect_timeout`, `tls_handshake_timeout`,
`keep_alive`, `shutdown_timeout`, and `shutdown_signal`. It binds TCP, Unix domain
sockets (`bind_uds`), h2c, rustls (several versions), and OpenSSL itself.

### Routing, guards, and app state

Routes are matched by `actix-router`, with route macros from `actix-web-codegen`.
Guards in [`actix-web/src/guard/mod.rs`](https://github.com/actix/actix-web/blob/ae1d51adc04ae0a8cc06b396329fea67cbd4483d/actix-web/src/guard/mod.rs) pick a
route by method, header, query, `Host` (`guard/host.rs`), or `Accept`
(`guard/acceptable.rs`), and combine with `Any` and `All`. State is `web::Data<T>`,
an `Arc`.

### Middleware

From `actix-web/src/middleware/`: `Logger` with a format string and path exclusions
(`logger.rs`), `Compress`, `DefaultHeaders`, `Condition` (a middleware that is on or
off), `ErrorHandlers`, which rewrites a response by status (`err_handlers.rs`),
`NormalizePath`, which trims, merges, or adds trailing slashes (`normalize.rs`), and
`from_fn`.

### Proxies

`ConnectionInfo` in [`actix-web/src/info.rs`](https://github.com/actix/actix-web/blob/ae1d51adc04ae0a8cc06b396329fea67cbd4483d/actix-web/src/info.rs) reads
`Forwarded` (RFC 7239), `X-Forwarded-For`, `X-Forwarded-Host`, and
`X-Forwarded-Proto`. Its documentation warns not to use `realip_remote_addr` for
security unless the headers cannot be spoofed, because it trusts whatever the client
sends.

### Static files

`actix-files` serves a directory (`Files`) or one file (`NamedFile`). From
[`actix-files/src/named.rs`](https://github.com/actix/actix-web/blob/ae1d51adc04ae0a8cc06b396329fea67cbd4483d/actix-files/src/named.rs) and `files.rs`: an `ETag`
and `Last-Modified`, `If-Match`, `If-None-Match`, `If-Modified-Since`, a single byte
range with `If-Range`, `Accept-Ranges: bytes`, a `Content-Disposition`, hidden files
refused unless enabled, optional directory listings, and precompressed `.gz`, `.br`,
and `.zst` siblings (`try_compressed`). `path_buf.rs` decodes the path once and
refuses a `%2F` that would add a segment.

### Uploads

`actix-multipart` has a low-level `Multipart` field stream and a derived
`MultipartForm`. From [`actix-multipart/src/form/mod.rs`](https://github.com/actix/actix-web/blob/ae1d51adc04ae0a8cc06b396329fea67cbd4483d/actix-multipart/src/form/mod.rs):
`MultipartFormConfig` caps the whole form (`total_limit`, 50 MiB by default), what is
held in memory (`memory_limit`, 2 MiB), and the number of parts (`max_parts`). A field
can set its own `limit`, and the lower limit wins. A duplicate field is ignored,
denied, or replaces the first. `form/tempfile.rs` streams a part to a `NamedTempFile`
in a chosen directory and keeps its client-given file name, content type, and size.

### Sessions and identity

`actix-session` stores state in a signed or private cookie
(`storage/cookie.rs`, under 4 KB) or in Redis (`storage/redis_rs.rs`). Its
`SessionLifecycle` is a browser session or a persistent cookie, with a
`TtlExtensionPolicy` of every request or only on change (`config.rs`). `Session` has
`renew` and `purge` (`session.rs`).

`actix-identity` sits on a session and holds a signed-in user id, with `login`
and `logout`. From [`actix-identity/src/config.rs`](https://github.com/actix/actix-extras/blob/c5c41b74d38345e081720ff42163bbd598032e6b/actix-identity/src/config.rs):
`login_deadline` signs a user out a fixed time after signing in, whatever they do,
and `visit_deadline` after a time with no visit. Both are off by default.

### Authentication

`actix-web-httpauth` parses `Authorization: Basic` (RFC 7617) and `Bearer` (RFC 6750)
into extractors, and builds `WWW-Authenticate` challenges with `realm`, `scope`,
`error` (such as `invalid_token`), `error_description`, and `error_uri`
([`headers/www_authenticate/challenge/bearer/challenge.rs`](https://github.com/actix/actix-extras/blob/c5c41b74d38345e081720ff42163bbd598032e6b/actix-web-httpauth/src/headers/www_authenticate/challenge/bearer/challenge.rs)).
`HttpAuthentication::basic`, `bearer`, and `with_fn` in `middleware.rs` run a
validator closure. Verifying the credential is left to the service.

### CORS

`actix-cors` ([`builder.rs`](https://github.com/actix/actix-extras/blob/c5c41b74d38345e081720ff42163bbd598032e6b/actix-cors/src/builder.rs)) has origin lists and
an origin function, `supports_credentials`, `max_age`, `expose_headers`,
`allow_private_network_access` (the draft Private Network Access header), and
`block_on_origin_mismatch`, which refuses the request itself rather than only leaving
out the CORS headers.

### Rate limits

`actix-limitation` ([`lib.rs`](https://github.com/actix/actix-extras/blob/c5c41b74d38345e081720ff42163bbd598032e6b/actix-limitation/src/lib.rs)) counts a fixed
window in Redis with one atomic pipeline: `SET key 0 EX period NX`, `INCR`, `TTL`.
The key comes from a closure, a cookie, or a session value. Over the limit is a bare
`429` with no `Retry-After` and no rate-limit headers, and a Redis error is a `500`,
so it fails closed (`middleware.rs`). `actix-governor` uses the GCRA algorithm in
memory, with a burst size and a key from the peer address or a function of the
service's own.

### WebSockets

`actix-ws` gives a `WebSocket` extractor with a `Session` to send text, binary, ping,
pong, and close, and an aggregated stream that joins continuation frames up to
`max_continuation_size` (`aggregated.rs`). A JSON codec is in `codec/json.rs`. The
heartbeat is the service's own loop, shown in the crate example in `lib.rs`.

### Observability

`tracing-actix-web` opens one root span per request through a `RootSpanBuilder`
(`root_span_builder.rs`), mints a request id (`request_id.rs`), and, behind a feature,
continues an incoming W3C trace context and sets the span's OpenTelemetry parent
(`otel.rs`).

### Configuration

`actix-settings` reads a TOML file with environment overrides into the server's
settings: hosts, mode, compression, logging, workers, backlog, connection limits,
keep-alive, timeouts, and TLS (`src/defaults.toml`).

### Testing

`actix-web/src/test/` has `init_service`, `call_service`, `TestRequest`, and
`read_body_json` for in-process tests. `actix-test` starts a real server on a port
with an `awc` client.

### Other

`actix-protobuf` is a Protobuf extractor and responder. `web::Form`, `web::Json`, and
`web::Payload` each have a config with a size limit and an error handler
(`types/`).

## Compared with axa

| Area | Actix Web and actix-extras | axa |
|---|---|---|
| Shape | A framework: server, router, macros, `App` builder | A library on Axum: plain functions, no macros, no hooks |
| Server | Own server, many workers, TLS, h2, Unix sockets | `hyper` over HTTP/1 only, TLS left to a proxy (`src/server.rs`), with header-read, keep-alive, send-stall, and connection limits |
| Graceful shutdown | `shutdown_timeout`, `shutdown_signal` | `SHUTDOWN_TIMEOUT_SECONDS`, shared by requests and jobs, `SIGTERM` and `SIGINT` tested |
| Client address | `ConnectionInfo` believes `Forwarded` and `X-Forwarded-*` from anyone | `TRUSTED_PROXIES`, rightmost untrusted entry, capped walk. Better. No `Forwarded`, `X-Forwarded-Proto`, or `X-Forwarded-Host` |
| Request logging | `Logger` format string, logs the path | Route template only, never the path or query. Better for privacy |
| Request id | `tracing-actix-web` mints one | Kept if safe, or minted, always returned |
| Errors | `ResponseError`, `ErrorHandlers` by status | RFC 9457 problems, and a bare status or rejection becomes one |
| Trailing slashes | `NormalizePath` | A group's `/` at the prefix. `tower-http` has `NormalizePathLayer` |
| Static files | `actix-files`: ranges, `ETag`, precompressed | None. `tower-http`'s `ServeDir` does ranges, `ETag`, and precompressed files |
| Uploads | `actix-multipart`: total, memory, part, and field limits, temp files | `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT` and `body_limit` only. Axum's `Multipart` parses fields but has none of these limits or temp files |
| Sessions | Cookie or Redis store, lifecycle and TTL policy | `axa-web-session`: Postgres or memory, encrypted at rest, signed, nothing stored before sign-in, `sign_in` rotates the id, form tokens. Stronger defaults |
| Session deadlines | `actix-identity` `login_deadline` and `visit_deadline` | Idle `lifetime` and cookie `max_lifetime`. No deadline counted from sign-in |
| Bearer auth | Parsing and `WWW-Authenticate` challenges, no verification | Keycloak and PASETO verification with ABAC. Its `401` carries no `WWW-Authenticate` |
| Basic auth or a static token | `actix-web-httpauth` | None. `tower-http`'s version is deprecated and compares with `==` |
| CORS | Origin list or function, private network access, block on mismatch | Listed origins, `*` refused in production |
| Rate limits | Redis fixed window, fails closed with a bare `429`; GCRA in memory | In memory, `Retry-After` and `RateLimit-*`, keyed hashes, bounded memory, lockout on `401`. Better, but per process only |
| WebSockets | `actix-ws` with aggregation and a JSON codec | Upgrades pass (`SRV-9`). Axum has `axum::extract::ws`. No way for a handler to hear that shutdown has begun |
| Server-sent events | Not in these crates | Axum has `axum::response::sse` |
| Tracing | Continues an incoming `traceparent` | Exports, but starts each trace at its own edge on purpose (`axa-opentelemetry`) |
| Configuration | TOML file plus environment | Environment only, production refuses unsafe settings |
| Testing | `actix-web::test`, `actix-test` | `axa-test` over `axum-test`, the whole stack, no port |
| Protobuf | `actix-protobuf` | None. `axum-extra` has a `Protobuf` extractor |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| `WWW-Authenticate` on every `401` from `axa-keycloak-auth` and `axa-paseto-auth` | RFC 9110 says a `401` must carry one, and RFC 6750 defines `Bearer` with `error="invalid_token"`. Clients and gateways use it to tell a missing token from a bad one | S | Done in 0.11 (T-1, PST-12, KCK-13) |
| A signal handlers can await when shutdown begins | A WebSocket or SSE stream holds its connection until `SHUTDOWN_TIMEOUT_SECONDS` cuts it, as `hyper`'s graceful shutdown does not reach an upgraded connection. With a signal, a handler can send close code `1001` or end its stream | S | Done in 0.11 (T-3, SRV-16) |
| Multipart upload limits and temp files | Axum parses fields but leaves part counts, per-field limits, memory caps, and spooling to disk to each service. These are easy to get wrong and generic | M | Medium |
| A shared store for `axa-request-rate-limit` | In-process limits multiply by the number of replicas. A Redis window lets a limit or a lockout hold across them | M | Medium |
| A static credential guard for operator routes | Metrics, docs, and admin routes sometimes need a fixed token or Basic password. `tower-http`'s is deprecated and not constant-time | S | Medium |
| A deadline counted from sign-in in `axa-web-session` | An idle limit alone lets an active session live for ever. OWASP asks for an absolute limit too, as `actix-identity`'s `login_deadline` gives | S | Medium |
| `X-Forwarded-Proto` and `Forwarded` from trusted proxies | Tells a handler the scheme the client used. `HOST` covers absolute URLs, so the need is small | S | Low |
| An optional `Host` allow-list | Refuses requests for names the service does not serve, against Host-header injection | S | Low |
| Continuing an incoming `traceparent` from trusted peers | One trace across services. `axa-opentelemetry` leaves this out on purpose, so only if that decision is revisited | M | Low |
| An `Origin` or `Sec-Fetch-Site` check on cookie-authenticated writes | A second line beside `csrf_verify` for JSON endpoints, like `block_on_origin_mismatch` | S | Low |
| A burst size on rate limits | GCRA, as in `actix-governor`, allows a short burst. `tower_governor` already does this for Axum | M | Low |

**`WWW-Authenticate`.** Both crates build their own `Rejection` response
(`crates/axa-keycloak-auth/src/guard.rs`, `crates/axa-paseto-auth/src/lib.rs`). A
`401` should add `WWW-Authenticate: Bearer` with `error="invalid_token"` when a token
was sent and failed, and with no `error` when none was sent, as RFC 6750 section 3.1
says. A `realm` could come from the guard's configuration. The `detail` stays as it
is, and `error_description` should not quote the reason, which axa keeps fixed. No new
dependency.

**Shutdown signal.** `axa::server` would hold a `tokio_util::sync::CancellationToken`,
or a `tokio::sync::watch`, that `accept_until` cancels when told to stop, and expose
`server::stopping()` returning a future, or put the token on `AppContext`. A
WebSocket loop selects on it next to `recv`. This adds no hook: it is a value a
handler may read. Workers already hear of shutdown, so the same token could serve
both (`SRV-6`).

**Multipart uploads.** A module `axa::upload`, behind a feature, over
`axum::extract::Multipart`: `Limits { total, in_memory, parts, per_field }`, a
function that reads fields into a map of small text values and `TempFile`s spooled
with `tempfile` into a chosen directory, and refusals as problems: `413` for size,
`400` for too many parts or a duplicate field. It should keep the client's file name
only as untrusted text and never use it as a path. No derive macro, because axa
avoids code generation.

**Shared rate-limit store.** `axa-request-rate-limit` would take a store, in memory
by default and Redis behind a feature reusing `axa-redis-cache`'s pool. It keeps the
keyed hash, `Retry-After`, and `RateLimit-*`. Unlike `actix-limitation`, a Redis
error needs a stated choice, failing open or closed, set by the service and logged,
because a `500` on every request when Redis blips takes the service down.

**Static credential guard.** A function in `axa::middleware`, or a small crate,
`require_token(secret)` and `require_basic(user, password)`, returning a `from_fn`
layer. It compares with `subtle::ConstantTimeEq`, answers a `401` problem with
`WWW-Authenticate`, holds the secret in a type whose `Debug` hides it, and in
production refuses a secret shorter than a set length at boot, as `axa-web-session`
does with keys.

**Absolute session deadline.** `sign_in` would record when it ran in the session,
and the layer would end a session older than `SessionOptions::absolute_lifetime`,
with a production default such as 12 hours. It is the same check as
`enforce_login_deadline` in `actix-identity/src/middleware.rs`, inside the crate that
already owns `sign_in`.

## What axa should not copy

- **Its own server, workers, and TLS.** Axum runs on `hyper`, and axa leaves TLS and
  HTTP/2 to a proxy so that no step waits with no limit (`SRV-7`).
- **Route macros and the `App` builder.** axa's spec forbids macros and code
  generation, and `Routes` is plain data.
- **Static files, `NormalizePath`, WebSockets, SSE, typed headers, Protobuf, and
  `Condition`.** `tower-http` (`ServeDir`, `NormalizePathLayer`), Axum
  (`extract::ws`, `response::sse`), `axum-extra` (`TypedHeader`, `Protobuf`), and
  `Option<Layer>` already cover them.
- **`ConnectionInfo`'s trust in forwarded headers.** axa's `TRUSTED_PROXIES` is the
  safer design.
- **A `Logger` format string that logs the path.** axa logs the route template on
  purpose.
- **`actix-settings`' TOML file.** axa takes configuration from the environment only.
- **Cookie-stored session state.** It is limited to 4 KB, cannot be revoked server
  side, and axa's encrypted server-side store with nothing kept before sign-in is the
  stronger default.
- **`actix-limitation`'s bare `429` and `500` on a store error.** axa's limiter
  already answers better.
- **Directory listings.** Rarely wanted in a service, and they leak file names.
- **Guards as a routing mechanism.** Axum's method routers and middleware do this,
  and a `Host` guard is better as an allow-list than as routing.
