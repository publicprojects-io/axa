# Rocket

Rocket is a web framework for Rust: routes are declared with attribute macros, inputs are
checked by request guards, and the application is built and launched by the framework.
Its repository is [github.com/rwf2/Rocket](https://github.com/rwf2/Rocket). I read the
`master` branch at commit
[`3a54d07`](https://github.com/rwf2/Rocket/tree/3a54d079aef060a8f732bd04ea54b0581a604087)
of 28 December 2025, which is version `0.6.0-dev`; the latest release is `v0.5.2`. It has
about 25,800 stars and is dual-licensed MIT and Apache-2.0. Rocket does not use Axum. It
runs its own server over `hyper` 1 and `hyper-util` ([`core/lib/Cargo.toml`][cargo]),
with its own router, its own request and response types, and its own middleware model
(fairings). So this study does not look at how it uses Axum. It asks which of Rocket's
capabilities and framework-level ideas Axa could offer to Axum services as plain
library code, and which Axum, Tower, or `axum-extra` already give.

> **Since this was written**, Axa 0.11 added a database that cannot serve answers `503` with `Retry-After`; the stack sends a `permissions-policy`; `server::Shutdown` lets a handler wait for shutdown. Those rows below are marked "Done in 0.11", and the rest stand as written.

Links below are to files at that commit, under
`https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/`.

[cargo]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/Cargo.toml
[config]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/config/config.rs
[secret]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/config/secret_key.rs
[shield]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/shield/shield.rs
[policy]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/shield/policy.rs
[cookies]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/http/cookies.rs
[flash]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/response/flash.rs
[limits]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/data/limits.rs
[validate]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/form/validate.rs
[context]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/form/context.rs
[ferror]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/form/error.rs
[temp]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/fs/temp_file.rs
[fname]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/fs/file_name.rs
[fserver]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/fs/server.rs
[fairing]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/fairing/mod.rs
[state]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/state.rs
[sentinel]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/sentinel.rs
[fromreq]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/request/from_request.rs
[request]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/request/request.rs
[catcher]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/catcher/catcher.rs
[shutdown]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/shutdown/config.rs
[stream]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/response/stream/mod.rs
[sse]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/response/stream/sse.rs
[reqid]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/trace/subscriber/request_id.rs
[local]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/core/lib/src/local/client.rs
[dbpools]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/contrib/db_pools/lib/src/lib.rs
[dbconfig]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/contrib/db_pools/lib/src/config.rs
[dbdatabase]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/contrib/db_pools/lib/src/database.rs
[templates]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/contrib/dyn_templates/src/lib.rs
[tfairing]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/contrib/dyn_templates/src/fairing.rs
[ws]: https://github.com/rwf2/Rocket/blob/3a54d079aef060a8f732bd04ea54b0581a604087/contrib/ws/src/lib.rs

## How it is built

### Configuration: Figment and profiles

[`config/config.rs`][config] builds configuration with Figment from three layers, each
overriding the one before: built-in defaults, `Rocket.toml` (or the file named by
`ROCKET_CONFIG`), and `ROCKET_`-prefixed environment variables. A profile is chosen by
`ROCKET_PROFILE`, and with none set it is `debug` in a debug build and `release` in a
release build. Defaults differ by profile: `info` and `pretty` logging in debug,
`error` and `compact` in release. An application reads its own settings from the same
Figment with `extract`, so its own typed struct gets the same layers, profiles, and
error messages naming where a bad value came from.

The `secret_key` is the one setting that fails closed. [`config/secret_key.rs`][secret]
says that in the debug profile a missing or all-zero key is replaced at launch with a
random one, and in any other profile it is an error at launch. The key's `Debug` text
prints `[generated]` or a redacted form, and serialising the config writes zeros, so the
key does not leak through a config dump.

### Request guards and managed state

[`request/from_request.rs`][fromreq] defines `FromRequest`, which any handler argument
can implement. It returns `Success`, `Error` with a status, or `Forward`, which lets the
next matching route try. Managed state ([`state.rs`][state]) is a type map on the
application, read with `&State<T>`. [`sentinel.rs`][sentinel] runs a check at launch on
every type in a mounted route's signature; the built-in one aborts launch when a route
asks for `State<T>` and no `T` is managed, so the mistake is caught at boot rather than
on the first request.

Axum covers most of this. `FromRequestParts` is the request guard, and `State<T>` is
checked at compile time once `with_state` is called, so a sentinel for state is not
needed. `Extension<T>` is the case Axum leaves to run time: a missing extension answers
`500` on first use.

### Forms, limits, and validation

Forms are parsed from `application/x-www-form-urlencoded` and `multipart/form-data`
into nested structs with a derive. Field validators in [`form/validate.rs`][validate]
are plain functions: `len`, `range`, `one_of`, `contains`, `omits`, `eq`, `neq`, `ext`
(a file's content type), `with`, and `try_with`. Every field's errors are collected, not
just the first. [`form/error.rs`][ferror] maps them to a status: `413` when a size limit
was hit, `400` for an I/O error reading a form, and `422` otherwise.

[`form/context.rs`][context] adds `Contextual<T>`, a form guard that never fails. It
keeps every submitted value and every error by field name, and it serialises as a map,
so a template can re-render the form with what the visitor typed and what was wrong.

[`data/limits.rs`][limits] sets a body limit by kind of data: `form` 32 KiB, `data-form`
2 MiB, `file` 1 MiB, `json` 1 MiB, `string` and `bytes` 8 KiB, and `file/<ext>` per
file extension.

### File uploads and static files

[`fs/temp_file.rs`][temp] streams an uploaded file to a temporary file under
`temp_dir`, capped by `limits.file` or `limits.file/<ext>`. It is deleted when dropped
unless the handler calls `persist_to` or `copy_to`. [`fs/file_name.rs`][fname] keeps the
client's file name only in a sanitised form: `as_str` strips the extension, path
separators, and control characters, and the raw name is available only through
`dangerous_unsafe_unsanitized_raw`. `FileServer` in [`fs/server.rs`][fserver] hides
dotfiles by default and serves `index.html` for a directory.

### Cookies and flash messages

[`http/cookies.rs`][cookies] wraps the `cookie` crate. `add` sets `SameSite=Strict` and
`Secure` when every listener is TLS, unless the cookie says otherwise. `add_private`
encrypts and authenticates with the `secret_key`, and also sets `HttpOnly` and an expiry
one week away. [`response/flash.rs`][flash] sets a one-shot `_flash` cookie of kind and
message, which a later request reads and clears. The flash cookie is added with `add`,
not `add_private`, so a visitor can write its contents.

### Shields (security headers)

[`shield/shield.rs`][shield] is a fairing that adds headers to each response. The
default set is `X-Content-Type-Options: nosniff`, `X-Frame-Options: SAMEORIGIN`, and a
`Permissions-Policy` that blocks only `interest-cohort` ([`shield/policy.rs`][policy]).
It also offers `Referrer-Policy`, `Strict-Transport-Security`, `X-DNS-Prefetch-Control`,
and the deprecated `Expect-CT` and `X-XSS-Protection`, and a typed builder for
`Permissions-Policy` features. When every listener is TLS and the profile is not debug,
it turns on HSTS with a warning. It never replaces a header the handler set, and logs a
warning when it skips one. It has no `Content-Security-Policy` policy.

### Fairings

[`fairing/mod.rs`][fairing] defines hooks a fairing implements: `on_ignite` (may change
or abort the build), `on_liftoff`, `on_request`, `on_response`, and `on_shutdown`.
Shields, templates, and database pools are all fairings. In Axum, request and response
hooks are Tower layers, and ignition, liftoff, and shutdown are ordinary code in `main`.

### Errors and catchers

[`catcher/catcher.rs`][catcher] lets a service register a handler per status code and
path prefix. The default catcher answers HTML, or JSON when the `Accept` header prefers
JSON.

### Client address and request id

[`request/request.rs`][request] reads the client address from `ip_header`, which is
`X-Real-IP` by default, and falls back to the socket address. The header is believed
from any caller. [`trace/subscriber/request_id.rs`][reqid] gives each request span a
short id for log lines; I found no request id sent back to the client.

### Graceful shutdown and streams

[`shutdown/config.rs`][shutdown] starts shutdown on Ctrl-C and, on Unix, `SIGTERM` by
default. It then allows `grace` seconds for requests to finish, then `mercy` seconds for
connections to close, then forces the runtime down. A handler can take the `Shutdown`
future as an argument, and [`response/stream/mod.rs`][stream] shows an endless stream
ending itself when it fires, rather than being cut off at the end of `grace`.
[`response/stream/sse.rs`][sse] sends server-sent events with a heartbeat comment by
default. WebSockets are in [`contrib/ws`][ws] over `tungstenite`.

### Testing client

[`local/client.rs`][local] dispatches requests to the application without a socket,
in async and blocking forms. `Client::tracked` keeps cookies between requests, and
`tracked_secure` treats the connection as TLS so `Secure` cookies are sent.

### rocket_db_pools

[`contrib/db_pools`][dbpools] binds a named pool from configuration to a type with a
derive, and supports sqlx (Postgres, MySQL, SQLite), deadpool (Postgres, Redis),
MongoDB, and async Diesel. [`config.rs`][dbconfig] reads `url`, `min_connections`,
`max_connections`, `connect_timeout`, `idle_timeout`, and SQLite `extensions`.
[`database.rs`][dbdatabase] opens the pool in `on_ignite`, so launch fails if it cannot
connect, closes it in `on_shutdown`, and answers `503 Service Unavailable` when the
`Connection<T>` guard cannot get a connection from the pool in time.

### rocket_dyn_templates

[`contrib/dyn_templates`][templates] renders Tera, Handlebars, or MiniJinja templates
from `template_dir`, picking the engine by file extension and the content type by the
extension before it. [`fairing.rs`][tfairing] loads templates at ignition, failing
launch if they do not load, and in a debug build checks for changed files before each
request and reloads them.

## Compared with Axa

| Area | Rocket | Axa |
|---|---|---|
| Shape | Framework: macros, fairings, launch lifecycle, its own server | Library over Axum; no hooks or macros |
| Configuration | Figment: defaults, `Rocket.toml`, `ROCKET_` variables, profiles | Environment variables only, defaults in code, production refuses unsafe values |
| A service's own settings | Typed `extract` from the same Figment, same rules | The variable reader is private (`Vars` in `src/config.rs`), so a service parses its own variables by hand |
| Secret key | Random in debug, required otherwise | `axa-web-session` requires two distinct keys of a minimum length in production |
| Request guards and state | `FromRequest`, `State<T>`, sentinels at launch | Axum extractors and `State<AppContext>`, with `shared_store` for other values; state is compile-time checked |
| Validation | Field validators, all errors kept, `422` | `Validatable` over the `validator` crate; failures become `400` |
| Re-rendering a failed form | `Contextual<T>` keeps values and errors by field | None |
| Form extractor | Built in, urlencoded and multipart | `extract::Json`, `Path`, `Query` reject with problems; no `Form` |
| Body limits | Per data kind and per file extension | One limit, `413` as a problem, raised per route with `middleware::body_limit` |
| Uploads | `TempFile` to disk with caps, sanitised `FileName` | Axum's `multipart` feature is on; no upload helper |
| Cookies | Jar with `Strict` and `Secure` defaults; private cookies | Session cookies in `axa-web-session`; no general cookie helper |
| Flash messages | Unsigned one-shot cookie | None |
| Security headers | nosniff, `SAMEORIGIN`, minimal Permissions-Policy, optional others, no CSP | nosniff, `DENY`, `no-referrer`, HSTS, strict CSP in production; no Permissions-Policy |
| HSTS | On by itself when every listener is TLS outside debug | On in production by setting |
| Error pages | Catchers by status and path; HTML or JSON by `Accept` | RFC 9457 problem details always |
| Client address | `X-Real-IP` believed from anyone | `X-Forwarded-For` only from `TRUSTED_PROXIES`, read from the right |
| Request id | Short id in log spans | Kept or minted, returned in `x-request-id`, on every log line |
| Graceful shutdown | Grace then mercy, then forced; `Shutdown` future for handlers | `SHUTDOWN_TIMEOUT_SECONDS` for requests and jobs; no signal a handler can wait on |
| Server limits | Keep-alive setting | Header-read, keep-alive, send-stall timeouts and a connection cap |
| Pool exhausted | `503` | `sqlx::Error` other than a broken constraint becomes `500`; I found no `PoolTimedOut` case in `src/error.rs` |
| Pool lifecycle | Opened at ignition, closed at shutdown | Opened by `axa::db::connect`; I found no explicit close in `src/boot.rs`, `src/cli.rs`, or `src/db.rs` |
| Templates | Three engines, reload in debug | Tera only, loaded once, escaping and page CSP enforced |
| SSE and WebSockets | Event streams with heartbeat; `rocket_ws` | Axum's `Sse` with `KeepAlive` and `WebSocketUpgrade` already cover these; Axa adds nothing |
| Testing | Local client, cookie tracking, blocking and async | `axa-test` over `axum-test`, the full stack with no port; `axum-test` has a cookie-saving option, which I did not confirm in its source |
| Health, jobs, outbox, metrics, OpenAPI | None in Rocket or its contrib crates | All present |

## What Axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| A public settings reader for a service's own variables | Rocket gives an application's own settings the same rules as its own. In Axa a service's own variables get none of the production refusal or secret redaction, which breaks P-3 at the edge | M | High |
| `503` with `Retry-After` when the pool has no connection | A full pool is overload, not a bug. `500` tells a load balancer and a client the wrong thing, and pages someone | S | Done in 0.11 (T-5, DB-7) |
| `Permissions-Policy` in the standard headers | Turns off camera, microphone, geolocation and similar for an API or a page that never uses them. The Kanidm study asks for the same, with COOP and CORP | S | Done in 0.11 (T-6, MID-20) |
| A shutdown signal a handler can wait on | An SSE stream or WebSocket never finishes, so today it holds every shutdown to its full timeout and is then cut off | S | Done in 0.11 (T-3, SRV-16) |
| `extract::Form` with problem rejection, and a form context | Axum's `Form` rejects with plain text that can quote input, as its `Json` does. A server-rendered form needs the submitted values and per-field errors back | M | Medium |
| An upload helper: a multipart field to a capped temp file, with a sanitised name | Every service with uploads writes this, and the file name and size checks are where it goes wrong | M | Medium |
| HTML error pages for browsers in `axa-tera-templates` | A Tera page that fails shows raw problem JSON to a visitor | M | Low |
| Reloading templates in development | Saves a rebuild per template change | S | Low |
| Flash messages in `axa-web-session` | The post, redirect, get pattern needs one; stored in the session, not an unsigned cookie as Rocket does | S | Low |
| Closing the pool at shutdown | Sends a clean terminate to the database rather than dropping sockets | S | Low |
| Body limits per content type | A JSON API and an upload route want different limits; `middleware::body_limit` per route already covers most of this | S | Low |

**A public settings reader.** Make the private `Vars` in `src/config.rs` public as
`axa::config::Settings`, built by `Settings::from_env(&environment)` or
`Settings::from_lookup(&environment, lookup)` for tests. It would offer `parsed(name,
default)`, `flag(name, default)`, `required(name)`, and `secret(name)` (a value never
printed), and `finish()`, which returns every problem at once as `Config::from_env`
does: refused in a production-like environment, warnings elsewhere, and values under
secret-shaped names never quoted (CFG-13, CFG-14). The service's own struct is then an
ordinary function over it, with no derive and no config files. `check` could print the
service's settings by taking `Settings::settings()` from the service. It needs no new
dependency.

**`503` for an exhausted pool.** In `src/error.rs`, map `sqlx::Error::PoolTimedOut` and
`PoolClosed` to a `503` problem with code `unavailable` and a short `Retry-After`, and do
the same for the Diesel pool's timeout and SeaORM's wrapped sqlx error, so the four
database crates still answer alike. Log it as a warning, not an error. It fits next to
`broken_sqlx`, needs no dependency, and should be a spec requirement with a test that
uses a pool of size one.

**`Permissions-Policy`.** Add it to the security-header layer in `src/middleware.rs`
with `set_default_header`, so a handler's own value still wins. Send a deny list of
powerful features (camera, microphone, geolocation, payment, USB and so on) by default
in every environment, with `HTTP_PERMISSIONS_POLICY` to name another and an empty value
to send none, matching `HTTP_CONTENT_SECURITY_POLICY`. A plain string is enough; Rocket's
typed builder is not needed.

**A shutdown signal for handlers.** `src/server.rs` already holds the shutdown future.
Put a cloneable `axa::server::Shutdown` (a `tokio::sync::watch` receiver or a
`CancellationToken`) on `AppContext`, fired when the drain starts. A stream ends itself
with `stream.take_until(ctx.shutdown.wait())`, and the drain then finishes early instead
of waiting out `SHUTDOWN_TIMEOUT_SECONDS`. Axum's `Sse` and `KeepAlive` stay as they
are; Axa only adds the signal.

**`extract::Form` and a form context.** Add `Form<T>` to `src/extract.rs` beside `Json`,
rejecting with a problem that quotes nothing, and running `Validatable` when `T`
implements it. For HTML forms, add `FormContext` to `axa-tera-templates`: the submitted
values (secret-shaped fields left out) and each field's messages from
`ModelValidationErrors`, serialisable for a template. Answer `422` with the re-rendered
page. It depends on `serde_urlencoded`, which Axum already brings in.

**An upload helper.** A function, not an extractor: `upload::to_temp_file(field,
max_bytes, dir)` takes an Axum `multipart::Field`, streams it to a `tempfile` that is
deleted unless persisted, stops at `max_bytes` with a `413` problem, and returns a
sanitised file name modelled on Rocket's `FileName::as_str`, with the raw name behind a
method whose name says it is unsafe. It could live in `axa` behind a feature, and would
add `tempfile`.

## What Axa should not copy

- **Config files and profiles.** Figment layers `Rocket.toml` under the environment,
  and profiles let a file say what each environment gets. Axa's spec forbids
  configuration files, and `APP_ENV` already chooses the defaults. What is worth taking
  is the typed reader, above, not the layering.
- **Fairings and sentinels.** Both are hooks the framework drives. Axum's Tower layers
  already do what `on_request` and `on_response` do, and launch and shutdown are code in
  the service's `main`. With `State<AppContext>` checked at compile time, a launch-time
  check for missing state is not needed.
- **Route macros, `Forward`, and route ranks.** Matching a route by trying guards in
  order hides which handler answers. Axa forbids route macros, and Axum's router answers
  exactly one route per path and method.
- **Catchers by path, and HTML by default.** Axa answers every error as a problem so
  that one shape reaches every client. An HTML page for browsers belongs in the templates
  crate, as an opt-in.
- **Believing `X-Real-IP` from anyone.** Rocket's default records a forged address.
  Axa's `TRUSTED_PROXIES` rule is better.
- **Turning HSTS on because TLS was detected.** Axa terminates no TLS, and states HSTS
  per environment, which is clearer than a side effect of how the server was bound.
- **Unsigned flash cookies.** A visitor can write Rocket's `_flash` cookie. A flash
  message in Axa would live in the session.
- **A general private-cookie jar.** `axum-extra` already has `PrivateCookieJar` and
  `SignedCookieJar`, and `axa-web-session` covers signed sessions with keys required in
  production. A second cookie helper would have no second consumer (P-2).
- **Several template engines.** One engine with enforced escaping and a page CSP is
  easier to audit than three.
- **A blocking test client.** `axa-test` is async over `axum-test`, and services test with
  `#[tokio::test]`.
- **SSE heartbeats and a WebSocket crate.** Axum's `Sse` with `KeepAlive` and
  `WebSocketUpgrade` already do this.
- **Its own server and TLS listeners, including mTLS.** Axa's server already has more
  connection limits than Rocket's, and TLS sits in the proxy in front.
