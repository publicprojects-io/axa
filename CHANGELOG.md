# Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `DbCommand::destroys_data` and `cli::DESTROY_DATA_VAR`.

### Changed

- `cli::parse` now refuses `db down`, `db reset`, and `db truncate` unless the process
  runs in development or test and `AXA_DESTROY_DATA` is `true`. It exits before the
  service runs anything, and `--environment` cannot lift the refusal. A hand-written
  `run` could leave the guard out, and one did. `Cli::parse` does not refuse them.

## [0.4.0] - 2026-11-06

This covers `axa` 0.4.0, `axa-paseto` 0.3.0, `axa-seaorm` 0.2.0, `axa-sqlx` 0.1.1,
`axa-environment` 0.1.1, and the first releases of `axa-keycloak`, `axa-tracing`, and
`axa-opentelemetry`, each 0.1.0. It breaks 0.3.0 where noted: `Defaults` has new fields, and the default database
timeouts are longer. See [Upgrading to axa 0.3](help/upgrading-to-0.3/index.md), whose
last section covers 0.4.

### Added

- **With the `sqlx` feature, `AppContext::db` is a `PgPool`**, and `/_readiness` asks
  it as `database`. This breaks a service on that feature, which passed `()` to
  `AppContext::new`. `db::connect` opens the pool from `config::Database`, and
  `PoolConfig` converts from it. With `sea-orm` too, `db` stays the SeaORM connection.
- `axa_sqlx::Db`, so that one function takes the pool or an open transaction. It has
  one service so far, the electronic patient record, where it was written, and
  `impl Acquire` futures are not `Send`.
- `Environment::may_destroy_data`, which a service calls before `db reset` or
  `db down`. `Cli` only parses those commands, so a hand-written `run` could leave
  the guard out, and one did.

- `Defaults::connect_timeout`, `Defaults::idle_timeout`, and `Defaults::auto_migrate`,
  so a service states these in code. `auto_migrate: false` suits a service sharing a
  database it does not own. Code building a `Defaults` without
  `..Defaults::default()` must set them.
- `config::DEFAULT_CONNECT_TIMEOUT` and `config::DEFAULT_IDLE_TIMEOUT`.
- `axa-keycloak`, for Keycloak bearer authentication and ABAC enforcement, behind
  the `keycloak` feature. It stands alone so that it can be run and audited by itself,
  with copies of the code it needs from `authentication-verifier` and `axa-paseto`.
  A guard can accept PASETO tokens as well, through a verifier the service supplies.
- `axa-tracing`, behind the `tracing` feature: `Redacted` log fields, `SlowSpans`,
  a request id readable anywhere and kept across `spawn` and peer calls, and
  diagnostics for a future that blocks the runtime, a stalled runtime, and panics.
- `axa-opentelemetry`, behind the `opentelemetry` feature: OTLP export of traces and
  request metrics. Export is off until `OTLP_ENDPOINT` is set, and plain `http` to a
  remote collector is refused unless `OTLP_ALLOW_INSECURE` is set.
- `HEADER_READ_TIMEOUT_SECONDS`, 30 by default, drops a client that does not finish its
  headers. `server` now runs its own connection loop, because `axum::serve` cannot set
  this, and `hyper` and `hyper-util` become direct dependencies. They were already in
  the tree. `config::Server` gains `header_read_timeout_seconds`.
- `Queue::connect_existing` and `queue::SETUP_STATEMENTS`, for a database role that may
  not change the schema.
- `logger::init_with`, which installs more layers beside the stdout logs and can
  widen the default filter. Layers keep running when `LOG_ENABLE=false`.

### Fixed

- `--help` and `--version` no longer describe `axa`. They printed `axa 0.3.0` and this
  crate's description from every service, because Cargo fills those in for the crate
  that writes them. `cli::parse` takes the service's own, and `Cli::parse()` now
  answers with neither.
- A service running `start --all` no longer takes up to twice
  `SHUTDOWN_TIMEOUT_SECONDS` to stop. Requests and jobs now share one limit, from one
  moment. Job loops also hear at once, so they no longer claim new jobs while the
  server waits for its requests.
- `check` now prints `HTTP_CSP` and `HSTS_MAX_AGE_SECONDS`.
- A panic now answers with the same JSON as any other internal failure, and logs an
  error carrying the request id. It used to answer `Service panicked` as plain text.
- A package name with a hyphen, such as `case-folder`, no longer hides the service's
  logs. The default filter named `case-folder`, and the logs' target is `case_folder`.
- Logs carry colour only on a terminal, so a log file or pipeline gets no escape codes.
- `DB_MAX_CONNECTIONS=1` no longer leaves production with a smallest pool size of 2. An
  unset smallest size now follows the largest. A pool that cannot exist, with a largest
  size of 0 or a smallest above the largest, is now refused like any other invalid
  setting.
- The `sqlx` feature now gives `Error::Sqlx`, so a handler can use `?` on a query. Only
  the `queue` feature did.
- `axa-keycloak` now uses the strict validation policy of `axum-keycloak-auth`, and
  needs 0.8.4 of it. The legacy default passed a token with no audience, an ID or
  refresh token, and one dated in the future. Every token must now name a configured
  audience, so a Keycloak client may need an audience mapper.
- Production refuses a `DATABASE_URL` that could go in the clear, unless it stays on this
  machine or `DB_ALLOW_INSECURE` is set.
- `axa_paseto::parse_bool` returns `Option<bool>`, because a typo read as off, and off can
  be what turns enforcement off.
- `axa-paseto` no longer tells a refused caller why, and logs the reason instead.
- A server fault no longer logs the error's `Debug` text, which for a database error
  quotes the row's values.
- The SeaORM guide's manifests named `axa` 0.1 rather than 0.4.
- A layer inside `middleware::apply` now reads the request id from the request's
  header, cleaned, as the response carries it. A request with no id had one in the
  extensions and none in the header, so a second reader minted a different id.

### Changed

- **`DB_CONNECT_TIMEOUT` is now 5 seconds and `DB_IDLE_TIMEOUT` 10 minutes**, up from
  500 ms each, matching `axa-sqlx`. The old defaults failed requests after half a
  second of waiting for a connection, and closed idle connections twice a second.
  `axa_seaorm::PoolConfig::new` changes to match.
- `tower-http` is now 0.7, `cruet` 1, and `validator` 0.21.
- CI now proves that `axa-diesel` links no C database library (DSL-1), and the spec
  lists the clippy run on stable.

## [0.3.0] - 2026-11-06

This covers `axa` 0.3.0, `axa-tera` 0.2.0, and `axa-session` 0.1.0. It breaks 0.2.0:
see [Upgrading to axa 0.3](help/upgrading-to-0.3/index.md).

### Added

- `axa-session`, for web sessions over `axum_session`, behind the `session` feature.
  Production needs two keys, and gets signed `Secure` cookies and encrypted storage.
  Nothing is stored for an anonymous visitor. `sign_in` stores a session and renews
  its identifier, and `csrf_token` and `csrf_verify` protect forms.
- `SHUTDOWN_TIMEOUT_SECONDS`, 25 by default, so a hung request or job cannot hold a
  process until it is killed. `config::Server` gains a field, so code building one
  by hand must set it.
- docs.rs now builds with all features, so `queue`, `testing`, and the subcrates
  appear in the published documentation.

### Changed

- **`axa_tera::Templates::html` returns a `Page`**, which sends its own
  `content-security-policy`. `axa`'s production default blocks a page's stylesheets
  and images.
- Only a server fault logs as an error. A client's mistake, such as a `404`, used to
  log at error level with its full text, which can quote personal data.

## [0.2.0] - 2026-11-05

This covers `axa` and `axa-paseto` 0.2.0. It breaks 0.1.0: `Http` and `QueueOptions`
have new fields, and `axa-paseto` now enforces by default.

### Fixed

- A Redis outage no longer holds requests up. `get_or_load` used to wait out the
  client's reconnection retries, for seconds on each request. Every `axa-redis`
  call now has a time limit, `CacheConfig::timeout`, 500 ms by default.
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

- `axa_redis::Cache::memory`, the same cache held in one process, for tests and for
  development with no Redis.
- CI audits dependencies with `cargo audit` and `cargo deny`, workflows with
  `zizmor`, and the whole history with `gitleaks`. Actions are pinned to a commit
  and service images to a digest.
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
