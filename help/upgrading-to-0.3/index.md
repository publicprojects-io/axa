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

## After 0.4.0

- **`cli::parse` refuses `db down`, `db reset`, and `db truncate`** outside development
  and test, and everywhere without `AXA_DESTROY_DATA=true`, so a script that runs one
  must set it. `--environment test` does not get round this on a production process.
  `Cli::parse` does not refuse them, so a service that keeps it must call
  `Environment::may_destroy_data` itself.

## Checking your work

```sh
cargo run -- check
cargo test
cargo test -- --ignored --test-threads=1   # against a real Postgres
```

Then start the real binary with production settings, and check that it serves
requests, that pages load their stylesheets, and that it stops cleanly on `SIGTERM`.
