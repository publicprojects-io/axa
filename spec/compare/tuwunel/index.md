# tuwunel

tuwunel is a Matrix homeserver in Rust, and the maintained successor to conduwuit and
Conduit. Its repository is <https://github.com/matrix-construct/tuwunel>, which was
reachable, so this page is about tuwunel and not its predecessors. I read commit
[`a9da94f`](https://github.com/matrix-construct/tuwunel/tree/a9da94f15108f39d2b5e28dc09514ab683ecdd7f)
of 9 November 2026. The repository has about 2,750 stars and about 294,000 lines of
Rust in a workspace of crates: `core`, `database`, `service`, `api`, `router`, `admin`,
`macros`, `matrix`, and `main`. It runs one process with an embedded RocksDB and no
external database. Axum 0.8 serves the HTTP side: the `router` crate builds one
`axum::Router` from the Matrix endpoints, which are typed by a fork of `ruma`, and wraps
it in a `tower::ServiceBuilder` stack. It serves on TCP, Unix sockets, sockets passed by
systemd, and direct TLS. It is a product, not a library, so most of it is
Matrix-specific. This page looks for the parts that are not.

> **Since this was written**, axa 0.11 added `/_health/ready` answers `503` from the moment shutdown begins. Those rows below are marked "Done in 0.11", and the rest stand as written.

## How it is built

Links point at commit `a9da94f`.

| Area | What tuwunel does | Source |
|---|---|---|
| App state | `Services`, a registry of services (36 directories under `src/service`), each built by `Service::build` with an optional long-running `worker`. Handlers get a `State` that is a raw pointer to `Services` rather than an `Arc`, with a safety note | [`src/service/service.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/service.rs), [`src/api/router/state.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/api/router/state.rs) |
| Supervised workers | A manager runs each service's worker in its own task, catches its errors and panics, and restarts it after 2.5 seconds | [`src/service/manager.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/manager.rs) |
| Middleware | Optional Sentry, compression (zstd, gzip, brotli, each switchable), `SetSensitiveHeadersLayer` for `Authorization`, a trace span with the route template, its own request handler, client IP source, response body timeout, request body timeout, request timeout as `408`, `nosniff`, a CSP and `X-Frame-Options` only on HTML responses, CORS, body limit, and panic catching that counts panics in metrics | [`src/router/layers.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/router/layers.rs) |
| Request handling | Answers `503` once shutdown has begun. Runs every `PUT`, `POST`, `DELETE`, and `PATCH` in a spawned task, so a client that hangs up does not cancel a write halfway. Logs the matched route template where there is one, with query values replaced | [`src/router/request.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/router/request.rs) |
| Errors | One `Error` turned into Matrix `errcode` JSON. Server errors are logged, and clients get a sanitised message. An error from a remote server is passed on only for kinds about that resource, and a remote `401` becomes `400`, so a client does not take it as its own session failing | [`src/core/error/response.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/core/error/response.rs) |
| Configuration | `figment`: an optional TOML file named by `--config` or `TUWUNEL_CONFIG`, then `TUWUNEL_` environment variables with `__` for nesting, then `-O key=value`. Unknown and deprecated keys are reported. A reload checks that fixed fields did not change. The example file is generated from the config structs | [`src/core/config/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/core/config/mod.rs), [`src/core/config/check.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/core/config/check.rs), [`src/main/args.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/main/args.rs) |
| Client address | `http.ip_source` picks the header to believe. Loopback peers and listed trusted subnets fall back to scanning headers, for bridges on Docker networks | [`src/api/router/client_ip.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/api/router/client_ip.rs) |
| Auth | Matrix access tokens, appservice tokens, server signatures, OIDC, MAS, LDAP, and JWT login, with account lock and suspension checks per route | [`src/api/router/auth.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/api/router/auth.rs) |
| Rate limiting | Password attempts limited per account and per address, with a token reserved before verification and kept only on a wrong password. A loopback peer is not limited by address | [`src/service/login_ratelimit/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/login_ratelimit/mod.rs) |
| Outbound HTTP | One client per purpose, each with its own resolver, timeouts, and pool. The resolver drops DNS answers in `ip_range_denylist`, which by default covers private, loopback, and link-local ranges, and media downloads check redirects too | [`src/service/client/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/client/mod.rs), [`src/service/resolver/dns.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/resolver/dns.rs), [`src/core/config/network.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/core/config/network.rs) |
| Object storage | Local disk, S3, and GCS through `object_store`, with multipart uploads, streaming reads, and optional signed URLs | [`src/service/storage/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/storage/mod.rs), [`src/service/storage/provider.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/storage/provider.rs) |
| Uploads and downloads | `Content-Disposition` is `inline` only for a list of 26 media types (raster images with no SVG, audio, video, JSON, CSS, CSV, and plain text), and filenames are sanitised | [`src/core/utils/content_disposition.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/core/utils/content_disposition.rs) |
| Mail | Pooled SMTP through `lettre`, disabled without a connection URI, with `Auto-Submitted` set. A build without the `email` feature refuses an SMTP URI at startup | [`src/service/sendmail/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/sendmail/mod.rs), [`src/service/sendmail/stub.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/sendmail/stub.rs) |
| Database and migrations | RocksDB with column families. Migrations stamp a marker each, and a database newer than the code is refused | [`src/database/engine.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/database/engine.rs), [`src/service/migrations/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/migrations/mod.rs) |
| Observability | `tracing` with reload handles, so the log filter changes at run time. Optional OTLP traces, Sentry, flame graphs, and tokio-console. Tokio runtime and task metrics, read through admin commands rather than a Prometheus route. jemalloc as an option | [`src/main/logging.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/main/logging.rs), [`src/core/metrics/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/core/metrics/mod.rs), [`src/admin/debug/change_log_level.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/admin/debug/change_log_level.rs) |
| Health | No HTTP health route of its own. `--health-check` probes the running server's own listeners for `200` on `/_tuwunel/server_version`, over TCP or the Unix socket, for a container `HEALTHCHECK` with no `curl` | [`src/main/health.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/main/health.rs) |
| Process | systemd readiness, status, and watchdog, socket activation, signals (`SIGTERM`, `SIGQUIT`, `SIGUSR1`, `SIGUSR2`), in-place restart, hot reload of modules, maintenance and read-only modes | [`src/router/run.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/router/run.rs), [`src/router/serve.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/router/serve.rs), [`src/main/signals.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/main/signals.rs), [`src/main/restart.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/main/restart.rs) |
| Admin tooling | Admin commands sent as messages in an admin room or a console: backups, checkpoints, config show and reload, cache clearing, memory and runtime stats, restart, shutdown | [`src/admin/server/`](https://github.com/matrix-construct/tuwunel/tree/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/admin/server), [`src/admin/debug/`](https://github.com/matrix-construct/tuwunel/tree/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/admin/debug) |
| Testing | About 150 integration test files in `src/main/tests`. A fixture boots an isolated server on an ephemeral port with a database named for the test, waits for the listener, runs the exercise, and removes the database. `test_utils` records warnings a test emits, and can run a test in a child process. The `tests` directory adds the Complement suite and Playwright | [`src/main/tests/fixture/mod.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/main/tests/fixture/mod.rs), [`src/service/test_utils.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/test_utils.rs) |

## Compared with axa

| Area | tuwunel | axa |
|---|---|---|
| Middleware | Its own stack, much like axa's. Adds request and response body timeouts, a sensitive-header marker, and CSP only on HTML | Request id, tracing, panic, security headers, CORS, timeout, `304`, compression, body limit. CSP on every production response. Header-read and send-stall limits at the connection |
| Writes when a client hangs up | Spawned, so they finish | Dropped with the handler future. A sqlx transaction rolls back, but work outside it may be left half done |
| Requests during shutdown | `503` to new requests on open connections | Stops accepting, and lets requests in flight finish. From `src/health.rs`, readiness does not turn `503` after `SIGTERM`; I did not test it |
| Errors | Matrix `errcode` JSON, sanitised, with rules for relaying upstream errors | RFC 9457 problems, internal text never sent |
| Configuration | TOML, environment, and `-O` overrides, reloadable, with unknown-key warnings | Environment only, defaults in code, invalid values stop production, `check` prints the effective settings |
| Client address | Configurable header, trusted subnets | `TRUSTED_PROXIES`, read from the right of `X-Forwarded-For` |
| Rate limiting | Login throttles per account and address | Per client or key, and failed-login lockout |
| Outbound HTTP | Clients per purpose, with an address denylist in the resolver | None |
| Storage | Local, S3, GCS | None |
| Mail | SMTP service | None |
| Background work | Supervised service workers that restart | Job queues on three databases with audits, schedules, and heartbeats. No supervised long-running loops |
| Health | `--health-check` command only | Three Kubernetes probe routes with named checks |
| Metrics | Runtime and task metrics read through admin commands | Prometheus and OTLP |
| Log level | Changeable at run time | Set at boot |
| Process | systemd notify and watchdog, socket activation, Unix sockets, direct TLS | TCP, `SIGTERM` and Ctrl-C |
| Tests | Booted servers per test, warning capture | `axa-test` request helpers |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| An outbound HTTP client that refuses private addresses | Any service that fetches a URL a user gave (webhooks, avatars, imports) is open to server-side request forgery without it | M | High |
| Let writes finish when the client hangs up | A dropped handler can leave a write done in the database but not in a second system, or the reverse | S | Medium |
| A `health` command that probes the local listener | Distroless images have no `curl`, so a Docker `HEALTHCHECK` needs the binary to probe itself | S | Medium |
| Safe `Content-Disposition` for downloads | Serving a user's upload inline as HTML or SVG is stored cross-site scripting | S | Medium |
| A log capture for tests | Lets a test check that a refusal was logged and that a secret was not | S | Medium |
| Supervised long-running tasks | Services run loops beside the server (relays, listeners). One that panics should be logged and restarted with a delay, and stop at shutdown | S | Low |
| Readiness that fails once shutdown begins | Lets a load balancer stop sending traffic before the listener closes | S | Done in 0.11 (T-2, HLT-11) |
| Request body timeout | A slow upload on a route with a long time limit holds a slot | S | Low |
| Unix sockets and systemd notify | Useful for services behind a local proxy or run by systemd, rarely in Kubernetes | M | Low |
| Changing the log filter at run time | Helpful in an incident, but needs an authenticated admin route | S | Low |

**An outbound HTTP client that refuses private addresses.** A new crate, such as
`axa-http-client`, would build a `reqwest::Client` with a resolver that drops any
address in a denylist of private, loopback, link-local, and metadata ranges, as
tuwunel's [`src/service/resolver/dns.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/resolver/dns.rs)
does. It would also check IP literals and every redirect, set connect and total time
limits, and cap the response size. Production would refuse an empty denylist (P-3),
and a variable such as `HTTP_CLIENT_ALLOWED_RANGES` would open one range. The signed
webhook publisher suggested in the [Loco comparison](../loco/index.md) would build on it.
It adds `reqwest`, which must be raised first (P-6). It meets P-2 through its safety
case.

**Let writes finish when the client hangs up.** This could be a layer in
`axa::middleware`, opt-in per route group, such as `Routes::finish_on_disconnect()`,
that runs the handler in a spawned task and awaits it, so dropping the response future
does not drop the handler. It must stay inside shutdown's limit: tuwunel ends a
detached task with `503` once the shutdown timeout passes, in
[`src/router/request.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/router/request.rs),
and axa would tie it to `SHUTDOWN_TIMEOUT_SECONDS`. The request span must be carried
into the task. tuwunel does this for every non-safe method. axa could do the same,
but a per-route choice is easier to reason about for a handler that is already
cancel-safe.

**A `health` command.** `axa::cli` would gain `service health`, which reads the same
configuration, connects to `127.0.0.1` on `PORT`, asks `/_health/live` (or `ready`
with a flag) with a 5 second limit, and exits `0` only on `200`, as tuwunel's
[`src/main/health.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/main/health.rs)
does. It needs no new dependency: a plain HTTP/1.1 request over `std::net::TcpStream`
is enough.

**Safe `Content-Disposition` for downloads.** A function in `axa::format`, such as
`format::download(bytes, content_type, filename)`, would send `inline` only for a short
list of safe types (images other than SVG, plain text, PDF, audio, and video), send
`attachment` for everything else, sanitise the filename and encode it as RFC 6266
`filename*`, and add `nosniff` and a strict CSP. tuwunel's list in
[`src/core/utils/content_disposition.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/core/utils/content_disposition.rs)
is a starting point. No new dependency.

**A log capture for tests.** `axa-test` would offer `capture_logs(level) -> Guard`,
built on a `tracing-subscriber` layer set as the thread's default, and a way to read
the recorded events and fields, as tuwunel's
[`src/service/test_utils.rs`](https://github.com/matrix-construct/tuwunel/blob/a9da94f15108f39d2b5e28dc09514ab683ecdd7f/src/service/test_utils.rs)
records warnings. axa's own tests that check a secret is never logged would use it too.

## What axa should not copy

- **Everything Matrix.** Federation, room state, `ruma` types, UIAA, appservices,
  presence, push, and the admin room are the product.
- **TOML files and `-O` overrides, and reloading them.** axa reads only the
  environment, by design. A reload that must check which fields may not change is the
  kind of complexity that choice avoids.
- **The `Services` registry and service trait.** It is a dependency-injection
  container in all but name, which axa's spec forbids. Its `State` is a raw pointer
  justified by a safety note, which axa has no reason to need.
- **RocksDB and its migration markers.** axa services keep their data in Postgres,
  MySQL, or SQLite, where the job queue and outbox live too.
- **Admin commands as chat messages, in-place restart, and hot-reloaded modules.**
  These suit a self-hosted server with one operator. A service in Kubernetes is
  restarted by the orchestrator, and its operators use the CLI and tasks.
- **CSP only on HTML responses.** tuwunel serves user media and needs this exception.
  axa sends a strict CSP on every production response, which is safer for an API.
- **Trusting headers from loopback and listed subnets without configuration.** axa's
  `TRUSTED_PROXIES` reads `X-Forwarded-For` from the right and ignores it when unset,
  which is stricter.
- **Metrics only through admin commands.** axa's Prometheus and OTLP export are what a
  cluster reads.
- **Sentry, jemalloc, and tokio-console as built-in features.** A service can add
  these itself. They are not shared needs.
