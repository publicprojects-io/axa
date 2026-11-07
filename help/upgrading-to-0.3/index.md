# Upgrading to axa 0.3

This covers a service on 0.1 or 0.2. Most of the work is small, but several changes
compile cleanly and behave differently, so read
[What changes with no compile error](#what-changes-with-no-compile-error) first.

## What will not compile

| Change | Since | What to do |
|---|---|---|
| `config::Http` has `hsts_max_age`, `csp`, and `timeout_overrides` | 0.2 | Set them, if you build one by hand |
| `config::Server` has `shutdown_timeout_seconds` | 0.3 | Set it, if you build one by hand |
| `config::Logger` has `warnings` | 0.2 | Set it, if you build one by hand |
| `queue::QueueOptions` has `heartbeat_interval`, and more | 0.2 | Use `..QueueOptions::default()` |
| `cli::Command` has `Check` | 0.2 | Add an arm to an exhaustive `match`, calling `cli::check` |
| `axa_tera::Templates::html` returns a `Page` | 0.3 | Return it as before. Its text is `page.0` |

## What changes with no compile error

**A release build with no `APP_ENV` is production.** It used to be development. A
container that never set `APP_ENV` now needs `DATABASE_URL`, and gets production's
stricter behaviour.

**A setting that makes no sense stops a production boot.** `HTTP_HSTS=tru` used to
turn HSTS off without a word. Run the new binary's `check` command against each
environment's settings before you deploy it.

**Production sends a strict `content-security-policy`:**
`default-src 'none'; frame-ancestors 'none'`. That suits a JSON API and blanks an
ordinary page. A page rendered with `axa_tera::Templates::html` sends its own policy.
Any other HTML needs `HTTP_CSP`, or a `content-security-policy` header of its own.
Swagger UI from `axa-openapi` sets its own.

**`axa-paseto` enforces by default.** `GuardConfig::new` used to leave enforcement
off. A service relying on that now answers `401`. Call `.require_auth(false)` to keep
it off, which logs a warning.

**Logs carry the route template.** The `path` field now reads `/orders/{id}`, or
`unmatched` for a request that matched no route. A dashboard or alert that matches
real paths needs changing.

**Only a server fault logs as an error.** A `404`, `400`, or other client mistake
used to log at error level, with its text. An alert counting those will go quiet.

**`stale_minutes` measures silence, not running time.** A job reports in while it
runs, so a long job is no longer taken for lost. A job that hangs now reports in for
ever, and holds its loop until the process stops.

**A job that panics now fails** rather than ending its worker loop, so a queue that
had been quietly losing loops will start reporting failed jobs.

**Shutdown has a limit.** After 25 seconds, requests and jobs still running are cut
off. Raise `SHUTDOWN_TIMEOUT_SECONDS` if a service needs longer, and keep it under
your orchestrator's own limit.

## Deploying the queue

`Queue::connect` adds the `attempts` and `max_attempts` columns by itself, so the
service's database role needs permission to alter `axa_queue`.

**Do not run old and new workers together for long.** Workers from 0.1 send no
heartbeat, so a 0.3 worker takes any of their jobs running longer than `stale_minutes`
for lost, and may run it again. Stop the old workers first, or drain the queue.

## Upgrading to 0.4

Everything above applies, and `axa-paseto` is now 0.3. On top of it:

- **`config::Defaults` has `connect_timeout`, `idle_timeout`, and `auto_migrate`.** Use
  `..Defaults::default()`, or set them.
- **With the `sqlx` feature, `AppContext::db` is a `PgPool`**, so pass the pool to
  `AppContext::new` where you passed `()`. A pool you added to `/_readiness` as a named
  check is now asked twice, so remove it.
- **`DB_CONNECT_TIMEOUT` is 5 seconds and `DB_IDLE_TIMEOUT` 10 minutes**, up from 500 ms
  each. Set them to keep the old values. `axa_seaorm::PoolConfig::new` changes to match.
- **Use `axa::cli::parse` in place of `Cli::parse`.** `Cli::parse` still works, and
  `--help` and `--version` then say nothing about the service.
- **In production, `DATABASE_URL` must set `sslmode` to `require`, `verify-ca`, or
  `verify-full`**, or the service refuses to start. A loopback address or a socket is
  exempt, and `DB_ALLOW_INSECURE=true` is the way out for a network that encrypts its
  own traffic.
- **`config::Server` has `header_read_timeout_seconds`.** Set it where a service builds
  a `Server` by hand. `HEADER_READ_TIMEOUT_SECONDS` sets it, and is 30 by default.
- **`axa_paseto::parse_bool` returns `Option<bool>`.** `None` is a word it does not know,
  so refuse to start on it rather than reading it as off.
- **A refusal from `axa-paseto` no longer says why.** A `401` says `invalid token` and a
  `403` says `forbidden`, so a client matching on the old text must stop.
- **Call `Environment::may_destroy_data` before `db reset` or `db down`.** `Cli` only
  parses those commands, so nothing guards them unless a service does.

## Upgrading to 0.4.1

- **`cli::parse` refuses `db down`, `db reset`, and `db truncate`** outside development
  and test, and everywhere without `AXA_DESTROY_DATA=true`, so a script that runs one
  must set it. `--environment test` does not get round this on a production process.
  `Cli::parse` and `Cli::try_parse` refuse them too.

## Upgrading to 0.5

- **A policy rule with `obligations` now needs the service to declare them.** Both guards
  used to allow such a request and drop the condition. Now it is a `403` until the service
  calls `Guard::honouring`, and a handler reads the decision through `Allowed`.
- `config::Http` has `trusted_proxies`, and `config::Server` has `max_connections`. Code
  building either without `..` must set them.
- `axa_diesel::PoolConfig` has `wait_timeout`, 5 seconds from `PoolConfig::new`.
- The logs say `auth_refused` in place of `paseto_forbidden` and `keycloak_forbidden`, at
  `warn`, not `info`, and for every refusal. An alert on the old names needs changing.
- A service behind a proxy should set `TRUSTED_PROXIES`, take `axa::client::ClientAddr`,
  and stop reading `X-Forwarded-For` itself. Until it sets them, the address is the
  proxy's.
- `axa-redis` logs no longer name the key.
- The Loco guide used to say to call `into_app` and then add your layers, which was
  wrong. Check `App::router`: use `into_router`, add your layers, and call
  `middleware::apply` last, or your guard's refusals miss the standard stack.
- The server speaks HTTP/1 only, since 0.4.1. Put a proxy in front to offer HTTP/2.

To add `axa-tracing` or `axa-opentelemetry`, call `logger::init_with` in place of
`logger::init`, and add their middleware before `middleware::apply`, so that it runs
inside the request span:

```rust,ignore
let telemetry = axa::opentelemetry::start(&otlp_config)?;
let mut layers = telemetry.layers();
layers.push(Box::new(axa::tracing::SlowSpans::new(Duration::from_secs(1))));
logger::init_with(
    "my_service",
    &config.logger,
    layers,
    axa::opentelemetry::DIAGNOSTIC_DIRECTIVES,
);
let router = router
    .layer(from_fn(axa::tracing::track))
    .layer(from_fn_with_state(telemetry.http_metrics(), axa::opentelemetry::middleware));
```

Export is off until `OTLP_ENDPOINT` is set, unlike the services' own `observability.rs`,
which export to `localhost:4317` by default. Call `telemetry.shutdown()` at the end of
shutdown.

## Upgrading to 0.6

**Every error `axa` answers is now an RFC 9457 problem**, with the media type
`application/problem+json`. Successes stay bare JSON. A service's own `ApiResponse`
envelope is its own, and this does not change it, but a response from `axa::Error` next
to it does change.

| Was | Is |
|---|---|
| `error` | `code`, an extension member |
| `description` | `detail`, absent when there is nothing more to say |
| `errors` | `errors`, an extension member |
| | `type`, `title`, and `status`, always |

```json
{"type":"about:blank","title":"Bad Request","status":400,"detail":"name is required","code":"bad_request"}
{"type":"about:blank","title":"Not Found","status":404,"detail":"Resource was not found","code":"not_found"}
{"type":"about:blank","title":"Internal Server Error","status":500,"code":"internal_server_error"}
{"type":"about:blank","title":"Bad Request","status":400,"code":"validation","errors":{"name":[{"code":"length","message":"is required"}]}}
```

- `ErrorDetail::new` and `with_reason` are unchanged. Code naming the fields changes:
  `ErrorDetail { error, description, errors }` is now
  `ErrorDetail { code, detail, errors }`, which the compiler will point out.
- These compile and behave differently, so search for them. `Error::BadRequest` and a
  rejected JSON body say `bad_request`, not `Bad Request`. A `500` has no `detail`, so
  show `title`. A model's validation failure has a `code`, `validation`, where it had no
  reason.
- A client or test reading `error` or `description` must read `code` and `detail`. A
  client that checks for `application/json` before parsing an error must accept
  `application/problem+json` too.
- `axa_paseto::Rejection` and `axa_keycloak::Rejection` are structs, and answer as
  problems. Code destructuring `(status, text)` writes `Rejection(status, text)`. A `401`
  or `403` from them was plain text, and a client reading that text must read `detail`.
- `axa_versioning`'s `406` has `code` in place of `error`, and keeps `requested` and
  `supported`.
- A `408` had an empty body, and a failing `/_readiness` keeps `ok` and `failed`. An
  orchestrator reading the status needs no change.
- Any error sent as plain text or with no `content-type` is now a problem with no
  `detail`. That covers an unmatched path's `404`, a wrong method's `405`, and an
  extractor's rejection, and these had plain text or nothing. It also covers a handler
  answering `(StatusCode::CONFLICT, "why")`, and the text is dropped, so search for
  those and return an `Error`. A layer of your own that rewrites Axum's refusals can go.
- `axa-tera`'s failed render is `text/html`, where it was `text/plain`.
- `axa::problem` no longer needs the `problem` feature, which adds nothing now.
- `config::Server` has `keep_alive_timeout_seconds`. Set it where a service builds a
  `Server` by hand.
- An idle connection now lasts `KEEP_ALIVE_TIMEOUT_SECONDS`, 75 by default, rather than
  `HEADER_READ_TIMEOUT_SECONDS`. A load balancer keeping idle connections for over 75
  seconds needs a higher one.
- A violated unique or foreign key constraint answers `409`, not `500`, so a client or a
  test expecting the `500` must change. A handler that already maps these keeps working.
- Mark a task that loads fixtures with `Task::development_only`, so that it cannot run
  in production.
- Import `axa::extract::{Json, Path, Query}` in place of Axum's in handlers whose clients
  should be told what was wrong. Axum's rejections are problems with no `detail`.
- A paging helper of your own can go: `Page::bounds`, `Page::respond`, and `?` on
  `PageParams::resolve` replace it.

## Upgrading to 0.6.1

No code must change. Check for these if a client or test reads them:

- A rejected JSON body now says `payload_too_large`, `unsupported_media_type`, or
  `unprocessable_entity` in `code` at `413`, `415`, or `422`, not `bad_request`.
- A constraint violation through the model layer answers `409`, not `500`.
- An `axa::extract::Path` on a route that does not fit it answers `500`, not `400`.

## Checking your work

```sh
cargo run -- check
cargo test
cargo test -- --ignored --test-threads=1   # against a real Postgres
```

Then start the real binary with production settings, and check that it serves
requests, that pages load their stylesheets, and that it stops cleanly on `SIGTERM`.
