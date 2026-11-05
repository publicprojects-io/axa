# Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Fixed

- A job that panics now fails, and its worker loop carries on. It used to end the
  loop for good, leaving the job `running`.
- A long job is no longer taken for lost while it runs: it reports in every
  `heartbeat_interval`, and `stale_minutes` now measures silence, not running
  time. A run that was given up on can no longer overwrite the job's later state.

### Changed

- **`axa-paseto` enforces by default.** `GuardConfig::new` used to leave enforcement
  off, so a service that forgot it served every request. `require_auth(false)`
  turns it off and logs a warning.
- Logs carry a request's route template in `path`, such as `/unsubscribe/{token}`,
  and `unmatched` for a request that matched no route. They used to carry the real
  path, which can hold a token or a person's identifier.
- **Production now sends `content-security-policy: default-src 'none'; frame-ancestors 'none'`.**
  A service that serves pages must set `HTTP_CSP`, or an empty `HTTP_CSP` to send none.
- `Http` gains the `hsts_max_age` and `csp` fields, so code building one by hand must set them.

### Added

- `queue::enqueue_on`, which queues a job on the caller's transaction.
- `BackgroundWorker::max_attempts`, a worker's own limit on tries.
- `HSTS_MAX_AGE_SECONDS` and `HTTP_CSP`.
- In `axa-openapi`: `drift` and `dangling_refs` check a document against what
  `AppRoutes::mounted` lists, and the Swagger UI page sets its own
  `content-security-policy`, so the production default does not blank it.
- `server::run`, which runs a server, a job queue, or both, and stops them together.
- `--server-and-worker` is accepted for `--all`.
- `axa-paseto`, for PASETO bearer authentication and ABAC enforcement, behind the
  `paseto` feature. Signature checks stay in `authentication-verifier`.
- `axa-redis`, a Redis cache with mandatory expiry, behind the `redis` feature.
- `axa-openapi`, for OpenAPI documents and a Swagger UI pinned to one exact
  version with integrity hashes, behind the `openapi` feature.
- `axa-seaorm`, behind the `sea-orm` feature, which also holds the code formerly
  in `axa::model` and `axa::schema`. Those paths still work. It adds Loco's
  `query::condition()` and `Validatable`, so moving from Loco needs less rewriting.
- `axa-diesel`, for Diesel pooling, migrations, and test resets over
  `diesel-async`, with no `libpq`, behind the `diesel` feature.
- `axa-tera`, for Tera templates with escaping on and generic errors, behind the
  `tera` feature.
- A guide to moving SeaORM code from Loco.
- `axa-environment`, with no dependencies, holding `Environment` and the
  `APP_ENV` then `LOCO_ENV` lookup, so a migration crate can use it without
  depending on `axa`. `axa::Environment` and `axa::environment` still work.
- `AppRoutes::mounted`, `Routes::mounted`, and `Handler::methods`, listing every
  route as `(method, full path)` for a route census.
- `health::Checks` and `AppContext::health_checks`, for named checks behind
  `/_readiness`, which now names what failed and reuses one answer for 5 seconds.
  A service on sqlx adds its pool this way. The route answers `{"ok":true}` as
  before when all is well.
- A `check` command, with `cli::check`, `Config::settings`, and `config::redact`.
  It reads the settings axa knows about and connects to nothing, so it says
  nothing about a service's own settings or whether the database answers.
- `Config::apply_cli`, for the `start --binding` and `--port` overrides.
- `Routes::timeout` and `Routes::add_with_timeout`, for a time limit stated where
  a route is declared. It can be longer than the stack's. This sits beside
  `Http::timeout_overrides` and beats it.

### Changed

- **A release build with no `APP_ENV` or `LOCO_ENV` is now production, not
  development.** So it refuses to boot without `DATABASE_URL`, and binds `0.0.0.0`.
  A debug build is still development. A blank value now counts as unset, so
  `APP_ENV=` in a release build is production too, and a blank `APP_ENV` no longer
  hides `LOCO_ENV`.
- **A setting that is set but invalid now stops a production-like boot**, naming
  the variable and its value. This covers numbers, flags, `LOG_LEVEL`,
  `LOG_FORMAT`, `WORKER_MODE`, and `LOG_FILTER`. Elsewhere the default applies and
  `logger::init` writes a warning to stderr. A deployment that boots today with
  such a value will stop booting.
- Flags have an explicit false list: `0`, `false`, `no`, `off`. An unknown value
  now keeps the default, where it used to mean false.
- A blank setting counts as unset, and a blank `DATABASE_URL` in production is now
  refused rather than being passed on as an empty address.
- **The default local database user is now `postgres:postgres`, not `loco:loco`.**
  A service relying on that default URL must now set `DATABASE_URL`.
- `Command` has a new `Check` variant, so a service's `match` needs a new arm.
  `AppContext`, `Handler`, `Routes`, and `config::Logger` have new public fields, so
  building one with a struct literal needs them too.
- `Environment::parse("")` now follows the build, as an unset name does.

- Log lines carry the request id, method, and path, and each finished request is
  logged at `info`.
- The job queue records attempts. A job gets one by default, with
  `max_attempts` and `retry_backoff` to retry it. A job lost with its worker now
  fails once out of attempts, rather than being queued again forever.
- `axa_queue` keeps finished rows for `retention_days`, then deletes them.
- Added `middleware::body_limit` and `Http::timeout_overrides`, for limits on one
  route, and `queue::warn_if_unserved`.

## [0.1.0] - 2026-11-05

### Added

- Routes, middleware, health checks, errors, configuration, logging, graceful
  shutdown, the shared command line, tasks, background jobs, and a Postgres job
  queue, plus test helpers and transitional SeaORM support.
- Subcrates, each behind a feature of the same purpose: `axa-sqlx` for pooling,
  migrations, and test resets; `axa-pagination`; `axa-problem` for RFC 9457
  errors; `axa-prometheus`; and `axa-versioning`.
