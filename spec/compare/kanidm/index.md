# Kanidm

Kanidm is an identity management server: accounts, groups, passkeys, OAuth2 and
OpenID Connect, SCIM, LDAP, and RADIUS. Its repository is
[github.com/kanidm/kanidm](https://github.com/kanidm/kanidm). I read the `master` branch
at commit [`253e73a`](https://github.com/kanidm/kanidm/tree/253e73a56f51347a141fe47a2548faf05d5b9db3)
of 10 November 2026. It has about 5,600 stars, is licensed MPL-2.0, and is written in
Rust. Its HTTP server lives in `server/core/src/https`. It builds an Axum 0.8 `Router`,
with `tower-http` for tracing, timeouts, compression, and static files, `axum-extra` for
cookies, `axum-htmx` for its server-rendered UI, and `utoipa` for OpenAPI. It does not
use `axum::serve`: it runs its own accept loop over `hyper-util`, and terminates TLS
itself with `rustls`. This study looks at security hardening, auth, sessions, CSRF,
headers, TLS, and observability.

Links below are to files at that commit, under
`https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/`.

[mod]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/mod.rs
[mw]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/middleware/mod.rs
[sec]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/middleware/security_headers.rs
[cache]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/middleware/caching.rs
[comp]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/middleware/compression.rs
[trace]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/trace.rs
[err]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/errors.rs
[ext]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/extractors/mod.rs
[csrf]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/views/csrf.rs
[cookies]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/views/cookies.rs
[views]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/views/mod.rs
[signup]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/views/signup.rs
[js]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/javascript.rs
[v1]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/v1.rs
[oauth2]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/oauth2.rs
[generic]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/generic.rs
[apidocs]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/https/apidocs/mod.rs
[tcp]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/tcp.rs
[crypto]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/crypto.rs
[config]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/config.rs
[lib]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/core/src/lib.rs
[daemon]: https://github.com/kanidm/kanidm/blob/253e73a56f51347a141fe47a2548faf05d5b9db3/server/daemon/src/main.rs

## How it is built

### App state and routing

[`https/mod.rs`][mod] holds a `ServerState` with `&'static` handles to its read and
write query actors, an in-memory HS256 signer, the trusted proxy ranges, two prebuilt
CSP header values, the origin, the domain, and a `secure_cookies` flag that is false only
in integration tests. Routes are merged from modules (`oauth2`, `v1_scim`, `v1`, the
htmx `views`), and the UI routes are only mounted when the server role has a UI. Layers
are added in a deliberate order with comments explaining why: request timeout
(300 seconds, `408`), trace, then client address resolution outermost.

### Connections and TLS

Kanidm runs its own accept loop in [`https/mod.rs`][mod] rather than `axum::serve`:

- Each connection has a 30-second limit to send any data, to finish the TLS handshake,
  and to send a PROXY header. HTTP/1 has a 60-second header-read timeout and HTTP/2 a
  60-second keep-alive. Which HTTP versions are served is configurable (`HttpVersions`
  in [`config.rs`][config]).
- TLS is built in [`crypto.rs`][crypto] (`setup_tls`) with `rustls` and the `aws-lc-rs`
  provider, `with_safe_default_protocol_versions`, and a check that refuses RSA keys under
  2048 bits and EC curves other than P-256 and P-384 (`check_privkey_minimums`).
- Client certificates (mTLS) are optional: a directory of hashed CA certificates (`.0`)
  and CRLs (`.r0`) builds a `WebPkiClientVerifier` that still allows unauthenticated
  clients. A verified peer certificate and its SHA-256 public key digest go into
  `ClientConnInfo`, which auth then reads.
- The TLS acceptor reloads without a restart: `SIGHUP` in
  [`server/daemon/src/main.rs`][daemon] sends `CoreAction::Reload`, a task in
  [`lib.rs`][lib] rebuilds the acceptor, and each listener swaps it in over a broadcast
  channel.
- Shutdown breaks the accept loop. Connection tasks are spawned and not tracked, so I
  found no draining of requests in flight in the files I read.

### Client address

Two mechanisms, chosen in [`config.rs`][config] (`HttpAddressInfo`):

- PROXY protocol v1 or v2 ([`tcp.rs`][tcp]), parsed only from configured CIDR ranges,
  with a time limit. A `LOCAL` header (a load balancer health check) keeps the connection
  address.
- `X-Forwarded-For` from configured ranges ([`middleware/mod.rs`][mw],
  `ip_address_middleware`). It takes the first (leftmost) entry and answers `400` if it
  does not parse.

### Security headers

[`middleware/security_headers.rs`][sec] sets on every response:
`content-security-policy` (unless the handler set one), `x-content-type-options:
nosniff`, `permissions-policy: fullscreen=(), geolocation=()`, `referrer-policy: origin`,
`strict-transport-security: max-age=63072001`, `cross-origin-embedder-policy:
require-corp`, `cross-origin-opener-policy: same-origin`, and
`cross-origin-resource-policy: same-origin`. Apart from CSP, these overwrite whatever a
handler set.

The CSP is built at startup in [`https/mod.rs`][mod]. It hashes each bundled JavaScript
file with SHA-384 ([`javascript.rs`][js]) and lists the hashes in `script-src`, so only
those exact files run; a missing file stops startup. It also has `base-uri 'none'`,
`frame-ancestors 'none'`, and `worker-src 'none'`, but allows `'unsafe-eval'`. A second
policy without `form-action` is applied to the login and OAuth2 routes
([`views/mod.rs`][views]), with a comment explaining that Chrome applies `form-action` to
redirects after a form post, which breaks OAuth2.

### Caching and compression

[`middleware/caching.rs`][cache] has two layers: `dont_cache_me` (`no-store, no-cache,
max-age=0` and `pragma: no-cache`) and `cache_me_short` (`private, max-age=300`).
`dont_cache_me` covers the whole `/v1` API, including auth and logout ([`v1.rs`][v1]),
and the OAuth2 token, introspect, and revoke endpoints ([`oauth2.rs`][oauth2]). Public
keys and a few tokens are under `cache_me_short`.

Compression ([`middleware/compression.rs`][comp]) is `tower-http`'s `CompressionLayer`
at best quality with Brotli turned off, and only on static UI routes. The module explains why: the BREACH attack
reads secrets out of compressed, encrypted responses, so API responses that carry
secrets are never compressed.

### Auth and sessions

- [`extractors/mod.rs`][ext] has `VerifiedClientInformation`, which reads `Authorization:
  Basic` or `Bearer` (a compact JWS), falls back to a bearer cookie only when there is
  no header, adds the client address and certificate, and pre-validates the token
  against the database. Its `#[instrument(skip_all)]` is commented "to prevent leaking
  tokens to logs".
- [`views/cookies.rs`][cookies] makes every cookie `HttpOnly`, `SameSite=Lax`, `Secure`
  (outside tests), with a domain and path `/`, and has helpers for signed cookies (a JWS
  by the server's HMAC key) and for removing a cookie with the same domain and path.
- The HMAC key for short-lived login state is generated per process and never stored
  ([`https/mod.rs`][mod]). A bad signature logs a warning naming the likely causes: a
  restart, or a load balancer without sticky sessions.

### CSRF and abuse

- Forms rely on `SameSite` cookies. HTML partials that change state are only served to
  htmx requests, through `HxRequestGuardLayer` ([`views/mod.rs`][views]).
- [`views/csrf.rs`][csrf] is not a token check but a small proof of work for the public
  signup and account recovery forms ([`views/signup.rs`][signup]). The server sets a
  random nonce in a signed `SameSite=Strict` cookie; the browser must find a value whose
  HMAC over the email field, the value, and a timestamp has its low 18 bits zero, within
  a 30-second window. This makes scripted signups cost CPU.
- I found no request rate limit or body limit in the `https` module. Account lockout
  happens in the identity library, which I did not read.

### Errors

[`errors.rs`][err] has one `WebError` enum mapping the library's `OperationError` to
status codes: `401` with `WWW-Authenticate: Bearer`, `403`, `404`, `400` for schema and
validation errors, `409` with the conflicting attribute names for uniqueness, and `500`
otherwise. The body is the error serialised as JSON. OAuth2 errors follow the OAuth2
error format and carry `access-control-allow-origin: *`. There is no RFC 9457 body.

### Observability

- [`trace.rs`][trace] makes an `INFO` span per request with method, full URI, version,
  connection and client address, status, latency, and an operation id. For OpenTelemetry
  it sets the span status to error for any non-2xx response. Spans for `/` and `/status`
  are marked `defer` so they can be kept quiet.
- `kopid_middleware` ([`middleware/mod.rs`][mw]) makes an operation id (from the
  tracing-forest id or the OpenTelemetry span id) and returns it as a response header.
  A caller's own id is not accepted.
- `version_middleware` returns the server version as a header on every response.
- In debug builds only, `are_we_json_yet` asserts that every successful `/v1` response
  is `application/json`.

### Health and docs

`/status` ([`generic.rs`][generic]) answers a JSON `true` from a status actor. There is
one route, not separate liveness and readiness. [`apidocs/mod.rs`][apidocs] serves
Swagger UI and `openapi.json` from `utoipa` derives. `/robots.txt` disallows everything,
and the `.well-known` change-password and passkey endpoints are served.

## Compared with axa

| Area | Kanidm | axa |
|---|---|---|
| Server loop | Own `hyper-util` loop, per-connection timeouts, HTTP version choice | Own loop with header-read and keep-alive limits, stalled-client tests, and graceful drain |
| Graceful shutdown | Stops accepting; I found no drain of requests in flight | Drains in-flight requests with a grace period |
| TLS | In-process `rustls`, key-strength check, mTLS with CRLs, reload on `SIGHUP` | None in the HTTP server; I found no TLS termination in `src/server.rs`, so it expects a proxy |
| Client address | PROXY v1/v2 or `X-Forwarded-For` from trusted CIDRs, first entry taken | `X-Forwarded-For` from `TRUSTED_PROXIES`, read from the right, bounded to 32 hops; no PROXY protocol |
| Request id | Always minted, caller's ignored | Caller's kept after cleaning, or minted |
| Security headers | CSP, nosniff, Permissions-Policy, Referrer-Policy, HSTS (always), COEP, COOP, CORP; handler wins only for CSP | nosniff, `x-frame-options`, Referrer-Policy, optional HSTS, CSP (strict default in production); handler wins for all |
| CSP for scripts | SHA-384 hashes of bundled scripts, computed at startup | A fixed or configured policy string; templates set their own |
| Cache-Control on auth | `no-store` layer on the whole API and token endpoints | No `no-store` layer; `conditional::etag` honours `no-store` when a handler sets it |
| Compression | Static routes only, for BREACH | Whole stack when enabled |
| Cookies | `HttpOnly`, `Lax`, `Secure`, signed JWS helpers | `axa-web-session`: `HttpOnly`, `Lax`, `Secure`, signed session |
| CSRF | `SameSite`, htmx-only partials, proof of work on public forms | `csrf_token` and `csrf_verify` in `axa-web-session` |
| Auth | Basic, bearer JWS, bearer cookie, mTLS certificate | Keycloak and PASETO with ABAC |
| Errors | Hand-mapped JSON, `WWW-Authenticate` on `401` | RFC 9457 problem details, `409` and `422` mapping |
| Tracing | Full URI and client address in the span | Route template only; address kept out as personal data |
| Health | One `/status` | `/_health/live`, `/ready`, `/startup` with cached, bounded checks |
| Rate limits | None found in the HTTP layer | `axa-request-rate-limit` |
| OpenAPI | `utoipa` and Swagger UI | `axa-openapi-docs` with drift checks |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| A `no-store` layer for credential routes | Tokens, sessions, and login answers should never sit in a browser or proxy cache | S | High |
| Cross-origin and Permissions-Policy headers | COOP and CORP stop cross-origin window and resource attacks at no cost to an API | S | High |
| Compression that skips secret-bearing answers | BREACH: compressing a response that holds a token next to attacker text leaks it | S | Medium |
| PROXY protocol v1 and v2 from trusted ranges | Layer-4 load balancers (such as AWS NLB) give the client address only this way | M | Medium |
| CSP script hashes from files | Lets a server-rendered app drop `'unsafe-inline'` without nonces | M | Medium |
| Optional in-process TLS with reload and mTLS | Services without a proxy, and mTLS between services | L | Low |
| A version response header | Helps support match a response to a build | S | Low |
| Debug-only content-type check in `axa-test` | Catches an API route that answers with the wrong type | S | Low |
| Proof-of-work for public forms | Raises the cost of scripted signups | M | Low |

**A `no-store` layer.** Add `middleware::no_store()` in `axa`, a `from_fn` layer that sets
`cache-control: no-store` (and `pragma: no-cache` for old proxies) unless the handler set
its own, for a route or group with `Routes::layer`. `axa-paseto-auth` and
`axa-keycloak-auth` could apply it to any route that issues or refreshes a token, and
`axa-web-session` to any answer that sets a session cookie. It needs only `tower-http` or
plain Axum. `conditional::etag` already skips `no-store` answers, so the two fit.

**Cross-origin and Permissions-Policy headers.** Extend the security-headers step in
`middleware::apply` with `cross-origin-opener-policy: same-origin`,
`cross-origin-resource-policy: same-origin`, and a restrictive `permissions-policy`,
each with `set_default_header` so a handler's own value still wins. Each should have an
`HTTP_*` variable in `axa::config` like `HTTP_CONTENT_SECURITY_POLICY`, with an empty
value sending none. `cross-origin-embedder-policy: require-corp` breaks pages that load
third-party images, so it should be opt-in, not a default.

**Compression that skips secret-bearing answers.** `tower-http`'s `CompressionLayer`
takes a `compress_when` predicate. axa could pass one that refuses any response with
`set-cookie` or `cache-control: no-store`, and offer a route-level opt-out marker. This
keeps the current `HTTP_COMPRESSION` switch and costs nothing for static or public
answers. The rule should be written in the spec as a requirement with a test.

**PROXY protocol.** Add it to `axa::server` and `axa::client`, reusing `ProxyNet` and
`TRUSTED_PROXIES` (or a separate `PROXY_PROTOCOL_TRUSTED`). Before handing the stream to
hyper, read a v1 or v2 header with a time limit from trusted peers only, and set the
`ConnectInfo` address from it; a `LOCAL` header keeps the peer address. It could depend
on the `haproxy_protocol` crate that Kanidm uses, or on `ppp`, behind a feature flag.

**CSP script hashes.** In `axa-tera-templates`, a function that takes a list of static
file paths, hashes each with SHA-384, and returns a `script-src` fragment (and the
matching `integrity` attributes for templates). A missing file is an error at startup,
not at request time. It depends on `sha2` and `base64`.

## What axa should not copy

- **Leftmost `X-Forwarded-For`.** Kanidm takes the first entry from a trusted proxy. That
  entry is whatever the client wrote, unless the proxy replaces the header. axa reads
  from the right, which is the safer rule.
- **Full URI and client address in the request span.** Query strings can hold OAuth2
  codes and tokens, and addresses are personal data. axa logs the route template on
  purpose.
- **Ignoring the caller's request id.** axa keeps a cleaned caller id, which helps trace
  a request across services.
- **Headers that overwrite the handler.** Kanidm replaces everything but CSP. axa's rule
  that a handler's own value wins is easier to reason about.
- **`'unsafe-eval'` in the CSP and an unconditional HSTS.** Both are product choices for
  Kanidm's UI and its always-TLS deployment, not good library defaults.
- **The htmx request guard.** It suits Kanidm's htmx UI. A general service should use
  `csrf_verify`, which axa already has.
- **A per-process signing key for login state.** It forces sticky sessions behind a load
  balancer. Kanidm accepts that on purpose; axa's sessions should keep working across
  replicas.
- **The `WebError` mapping and the single `/status` route.** axa's problem details and
  three health routes already do more.
- **In-process TLS as a default.** It is worth offering as an optional crate, but most
  services run behind a proxy that terminates TLS, and certificate handling is a large
  surface to keep correct.
