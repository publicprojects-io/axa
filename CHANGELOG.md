# Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **A cross-site write guard in the standard stack** (MID-23 to MID-28): an unsafe
  method or a WebSocket upgrade with `sec-fetch-site: cross-site` is `403`, code
  `cross_site_write_refused`, and with no `sec-fetch-site`, an `origin` that is not the
  request's host is `403`, code `origin_mismatch`. A request with neither header passes.
  An origin `CORS_ALLOWED_ORIGINS` lists passes and gets its CORS headers, and the
  refusal carries the request id and security headers.
- **`Http::cross_site_paths`**, set in code, for a route that takes a cross-site write
  by design, such as an identity provider's `form_post` callback.
- **A `Tenant` extractor in `axa-paseto-auth` and `axa-keycloak-auth`** (PST-13 to
  PST-15, KCK-14, KCK-15): exactly one value of the token's `tenant` attribute, or the
  one `GuardConfig::tenant_attribute` names, read only from a caller the guard verified
  and allowed. No verified caller is `401`, and no tenant, two, or one that is not a
  lower-case DNS label is `403`, code `no_tenant`, with an `auth_refused` line.
- **`axa_postgres_sqlx::begin_with`** (SQX-8, SQX-9): a transaction with settings such
  as `app.tenant` set for its length only, bound, for Postgres row-level security. It
  sets only custom dotted names, and refuses `role`, `search_path`, and the like.
- **`axa_test::isolation`** (TST-3): `assert_hidden` sends probes of a resource as
  another tenant and fails, naming each, on any answer but `404`, a `403` among them,
  or a list that shows the owner's text.
- **`axa_redis_cache::Cache::for_tenant`** (RDS-8, RDS-9): a cache whose every key is
  one tenant's, under `prefix#tenant:`, which no key of the service's own or of another
  tenant can equal, so one tenant's cached answer is never served to another.
- **Row-level security settings and checks in the other Postgres crates** (SOR-5,
  DSL-6, TOA-4): `axa_postgres_seaorm::begin_with` and `axa_postgres_toasty::begin_with`
  give a transaction with the settings, and `axa_postgres_diesel::set_local` sets them
  inside a Diesel transaction, refusing a connection with none open. Each has
  `row_security`, so `require_row_security` works with any Postgres database feature.
  MySQL and SQLite have no row-level security, so their crates have neither.
- **`axa::db::require_row_security`** (DB-9, SQX-10), with any Postgres database feature: at boot,
  production refuses a role that is a superuser or has `BYPASSRLS`, and any named table
  without row-level security forced, as each would let a policy silently do nothing.
  Development warns. `axa_postgres_sqlx::row_security` gives the report.
- **Safe downloads in `axa::format`** (FMT-5 to FMT-8): `download` sends a body as a
  file, and `content_disposition` makes its header, `inline` only for a raster image,
  audio, video, or plain text, which cannot run script, and `attachment` for HTML, SVG,
  PDF, and anything else. `clean_file_name` strips the path, control and bidirectional
  characters, and quotes, and a name that is not ASCII is sent as `filename*` too.

### Changed

- **The standard stack decodes a compressed request body** (MID-29): `gzip`, `deflate`,
  `br`, and `zstd`, with the body limit counted on decoded bytes, so a small body that
  expands past it is a `413`. An encoding it cannot decode is a `415`. A handler that
  read a compressed body itself now gets it decoded, without `content-encoding`.
- **`axa_redis_cache::Error` has a new variant, `TenantScope`**, so a `match` on it
  without a wildcard arm adds one.
- **`GuardConfig` in both auth crates has a new field, `tenant_attribute`**, so a
  service that builds one by hand, rather than with `GuardConfig::new`, adds it.
- **`config::Http` has a new field, `cross_site_paths`**, so a service that builds an
  `Http` by hand adds `cross_site_paths: Vec::new()`.

## [0.12.1] - 2026-11-10

This covers `axa` 0.12.1 and the new `axa-maud-templates` 0.1.0. As in 0.12.0, the
`sqlite-*` features are left out of the published `axa` until the SQLite crates can be
published.

### Added

- **`axa-maud-templates` 0.1.0, behind the `maud` feature, as `axa::maud`**: Maud
  compile-time HTML templates, with what `axa-tera-templates` gives Tera. Values are
  escaped unless wrapped in `PreEscaped`, a `Page` sends its own content security policy,
  and a handler's `Error`, from any error through `?`, answers a generic `500`. Maud's
  `html!` names the `maud` crate, so a service adds `maud = "0.27"` beside `axa`.

## [0.12.0] - 2026-11-10

This covers `axa` 0.12.0; the Postgres crates under their new names, `axa-postgres-sqlx`
0.1.3, `axa-postgres-seaorm` 0.3.0, `axa-postgres-diesel` 0.2.1, and `axa-postgres-toasty`
0.1.0; the new `axa-mysql-sqlx`, `axa-mysql-seaorm`, `axa-mysql-diesel`, and
`axa-mysql-toasty`, all 0.1.0; and new versions of `axa-transactional-outbox` 0.3.0 and
`axa-test` 0.3.0. `axa-sqlx` 0.1.4, `axa-seaorm` 0.3.1, `axa-diesel` 0.2.2, and
`axa-toasty` 0.1.1 are the last under those names, published only to say where each went.

The SQLite crates are in the repository and not published, and the `sqlite-sqlx`,
`sqlite-seaorm`, and `sqlite-diesel` features are left out of the published `axa`
0.12.0, as crates.io takes no feature that names an unpublished crate. Both come once sqlx
releases its fix for RUSTSEC-2026-0459.

Read [Upgrading to Axa 0.12](help/upgrading-to-0.12/index.md): for a service on Postgres,
the upgrade is renaming.

### Added

- **Database crates for MySQL and SQLite**, each with the shape of its Postgres sibling:
  `axa-mysql-sqlx`, `axa-mysql-seaorm`, `axa-mysql-diesel`, `axa-mysql-toasty`,
  `axa-sqlite-sqlx`, `axa-sqlite-seaorm`, and `axa-sqlite-diesel`, all 0.1.0, each behind
  the `axa` feature of its own name. Each makes `AppContext::db` its pool, and a violated
  constraint and a database that cannot serve for now answer as they do on Postgres.
  `axa-mysql-diesel` is pure Rust, with no `libmysqlclient`, and every MySQL crate reads
  TLS from the URL's `ssl-mode`. There is no SQLite Toasty crate, as Toasty's SQLite
  driver needs a newer `libsqlite3-sys` than sqlx allows in one build. The SQLite crates
  build `libsqlite3-sys` 0.37, which has RUSTSEC-2026-0459, so they are not published
  until sqlx raises its cap on it.
- **Production checks a MySQL `DB_URL` for TLS too**: `ssl-mode` of `REQUIRED`,
  `VERIFY_CA`, or `VERIFY_IDENTITY`, unless it stays on this machine. A SQLite `DB_URL`
  is a file on this machine, and a database `axa` does not know fails closed.
- **A development build with no `DB_URL` uses a local database of the kind
  `AppContext::db` holds**: Postgres, MySQL, or a SQLite file.

### Changed

- **The Postgres database crates are named for their database**: `axa-sqlx` is
  `axa-postgres-sqlx`, `axa-seaorm` is `axa-postgres-seaorm`, `axa-diesel` is
  `axa-postgres-diesel`, and `axa-toasty` is `axa-postgres-toasty`, each at the version
  it had. Rename them in `Cargo.toml`, and `axa_sqlx::` and the like in code.
- **The database features are named for their database and library**: `sqlx` is
  `postgres-sqlx`, `sea-orm` is `postgres-seaorm`, `diesel` is `postgres-diesel`, and
  `toasty` is `postgres-toasty`, and their modules are `axa::postgres_sqlx` and the like.
  `axa-transactional-outbox` 0.3.0's `sea-orm` feature is `seaorm`, and `axa-test` 0.3.0
  is for `axa` 0.12. With more than one
  database feature on, Postgres wins, then MySQL, then SQLite, and for each, sqlx, then
  SeaORM, Diesel, and Toasty.
- **`axa::Error::Diesel` holds a `DieselError`**, the same for each database's Diesel
  crate, with `Query`, `Pool`, and `Other`. `?` still turns a Diesel crate's error, and
  Diesel's own query error, into it.
- **A SeaORM feature builds sqlx too**, which SeaORM builds anyway, so that its errors are
  read as sqlx's are, on every database.
- **The crate description is "Accessories library for Axum websites, databases,
  microservices, and more."**


This covers `axa` 0.11.0, `axa-problem` 0.2.0 under its old name again, the new
`axa-mysql-job-queue` 0.1.0 and `axa-toasty` 0.1.0, and new versions of `axa-seaorm`
0.3.0, `axa-environment` 0.2.0, `axa-paseto-auth` 0.6.0, `axa-keycloak-auth` 0.4.0,
`axa-request-rate-limit` 0.3.0, `axa-test` 0.2.0, `axa-sqlx` 0.1.3, `axa-diesel` 0.2.1,
`axa-idempotency-key` 0.2.2, `axa-accepts-version-negotiation` 0.2.1, and
`axa-prometheus-metrics` 0.1.1. `axa-sqlite-job-queue` 0.1.0 is in the repository and
not yet published. It breaks 0.10 in its settings, health routes, and more, so read
[Upgrading to Axa 0.11](help/upgrading-to-0.11/index.md) before deploying it.

### Added

- **`server::Shutdown`**, an extractor that says when shutdown has begun, and waits for
  it, so a WebSocket or a stream of server-sent events ends itself rather than being cut
  off at `SHUTDOWN_TIMEOUT_SECONDS`.
- **`SHUTDOWN_DRAIN_SECONDS`**, 0 by default: told to stop, the server keeps accepting for
  that long while `/_health/ready` answers `503`, so that Kubernetes takes the pod out of
  the Service before it stops listening. `SHUTDOWN_TIMEOUT_SECONDS` counts from its end.
- **`RequestMetrics` in `axa-prometheus-metrics`**: request durations by method, route
  template, and status, with one `unmatched` label for requests that match no route.
- **`HTTP_PERMISSIONS_POLICY`**, the `permissions-policy` the stack sends.

- **`axa-mysql-job-queue` 0.1.0**, the MySQL twin of `axa-postgres-job-queue`: the same
  job loops, limits, delays, priorities, status, schedules, and acts, on MySQL 8.4 or
  later, with the same API and tables. It is a crate on its own, and not yet a feature
  of `axa`. Setup creates two triggers, which with binary logging on needs `SUPER` or
  `log_bin_trust_function_creators`, so run `SETUP_STATEMENTS` in a migration as such a
  role. MySQL has no `TRUNCATE` trigger, so give the service's role no `DROP` on
  `axa_job_acts`.
- **`axa-sqlite-job-queue` 0.1.0**, the SQLite twin of `axa-postgres-job-queue`, for a
  service on one machine: the same API, tables, and states, on a file. A claim is one
  `UPDATE ... RETURNING`, and a schedule's tick a `BEGIN IMMEDIATE` transaction, so
  each job and each due time goes out once across processes. Its tests need no server.
  It is not published until sqlx releases the raised cap on `libsqlite3-sys`, as the
  copy sqlx 0.9.0 builds has CVE-2026-119375.
- **`axa-toasty` 0.1.0 and the `toasty` feature**, for Postgres over
  [Toasty](https://github.com/tokio-rs/toasty) 0.11. `AppContext::db` becomes a
  `toasty::Db`, and `axa::db::connect` takes the models as well as the configuration.
  Toasty migrations go forward only, so there is no `revert`.
- **The four database crates have the same shape**: `Pool`, `Error`, `PoolConfig`,
  `connect`, `ping`, `migrate`, `pending`, `truncate`, `dangerously_recreate`,
  `is_unique_violation`, and `quote_identifier`, and `revert` but for Toasty.
  `axa-sqlx` gains `Pool`, `Error`, and `pending`. `axa-seaorm` gains `Pool`, `Error`,
  `ping`, `migrate`, `revert`, `pending`, `truncate`, `dangerously_recreate`,
  `is_unique_violation`, and `quote_identifier`. `axa-diesel` makes `web_tls` public.
- **The `diesel` feature makes `AppContext::db` a Diesel pool**, which readiness
  checks, and `axa::db::connect` opens from configuration. `axa::Error` takes a Diesel
  error with `?`, and a broken constraint answers `409` or `422` as it does for sqlx.

### Changed

- **A `401` from `axa-paseto-auth` and `axa-keycloak-auth` carries
  `WWW-Authenticate: Bearer`**, as RFC 9110 requires. It has no `error` parameter, which
  would say why.
- **`/_health/ready` answers `503`, `code` `shutting_down`, from the moment shutdown
  begins**, whatever its checks say.
- **The body limit binds a handler that reads the body itself**, not only the extractors.
  `middleware::body_limit` now wraps a route, `body_limit(post(upload), bytes)`, in place
  of `post(upload).layer(body_limit(bytes))`, and raises both.
- **A database that cannot serve for now answers `503`**, `code` `unavailable`, with
  `Retry-After: 5`, in place of `500`: a pool with no free connection, a lost connection,
  or a read-only, shutting-down, or full server, through any of the four database crates.
- **The stack sends `cross-origin-opener-policy` and `cross-origin-resource-policy` of
  `same-origin`, and a `permissions-policy`** that turns off the camera, microphone,
  location, payment, USB, sensors, and ad topics. A handler's own values win. A service
  whose images or fonts another site loads sets its own `cross-origin-resource-policy`
  on those routes.
- **A response that sets a cookie is `no-store`** unless its handler said how to cache
  it, and compression skips any response that sets a cookie or is `no-store`, against
  BREACH.
- **`authorization`, `proxy-authorization`, `cookie`, and `set-cookie` are marked
  sensitive** in the standard stack, so no layer records them.
- **`config::Server` has `shutdown_drain_seconds`, and `config::Http` has
  `permissions_policy`**, so a service that builds either by hand adds the field.
- **The health routes follow Kubernetes' three probes**: `/_health/live`,
  `/_health/ready`, and `/_health/startup`, in place of `/_ping`, `/_health`, and
  `/_readiness`, which now answer `404`. Point each liveness, readiness, and startup
  probe, load balancer, and uptime check at its new route before upgrading.
  `/_health/live` asks nothing. `/_health/ready` asks what `/_readiness` asked: the
  database, the queue, and the named checks. `/_health/startup` is new, asks only the
  database, and once it passes, passes for good, answering `not_started` until then.
  `axa-paseto-auth` and `axa-keycloak-auth` leave the three paths public, and
  `axa-request-rate-limit` exempts them, in place of the old ones.
- **`axa::model` and `axa::validation` work with every database**, and come with any
  of `sqlx`, `sea-orm`, `diesel`, or `toasty`, through a new `model` feature they turn
  on. `ModelError` has a variant for each database's error: `DbErr`, as before, and
  `Sqlx`, `Diesel`, and `Toasty`. Code that names `axa::model` or `axa::validation`
  does not change.
- **`axa-seaorm` 0.3.0 no longer has `model` or `validation`**, which moved to `axa`.
  A service that depends on `axa-seaorm` alone and names `axa_seaorm::model` or
  `axa_seaorm::validation` uses `axa::model` and `axa::validation` instead.
- **With both `sqlx` and `sea-orm` on, `AppContext::db` is now the `PgPool`**, not the
  SeaORM connection, because sqlx is not an ORM. With more than one database feature
  on, `sqlx` wins, then `sea-orm`, `diesel`, and `toasty`. A service that turns on
  both and uses `ctx.db` as a `DatabaseConnection` turns off `sqlx`, or keeps its own
  SeaORM connection.
- **`axa-diesel`'s `PoolConfig` leaves the connection string out of its `Debug`
  text**, as the sqlx and SeaORM ones do. It printed the password before.
- **`AppContext`'s `Debug` text names the database by its type**, as a Diesel pool has
  no `Debug` text of its own.
- **`axa-problem-details-for-http-apis` is `axa-problem` again**, at 0.2.0, with the
  same code as `axa-problem-details-for-http-apis` 0.1.0. `axa::problem` is unchanged.
  A service that depends on the crate directly renames it in `Cargo.toml` and its
  `use axa_problem_details_for_http_apis` lines to `axa-problem` and `axa_problem`.
- **The database URL is read from `DB_URL`, not `DATABASE_URL`.** It sits with the
  other `DB_` settings. A service that still sets only `DATABASE_URL` has no database
  URL: production refuses to boot, naming `DB_URL`, and development falls back to its
  default database. The `#[ignore]` suites read `AXA_TEST_DB_URL` in place of
  `AXA_TEST_DATABASE_URL`. Rename both in every Dockerfile, deploy config, `.env`
  file, and CI workflow.
- **Two HTTP settings are renamed to say what they limit.** `BODY_LIMIT_BYTES` is now
  `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, and `HEADER_READ_TIMEOUT_SECONDS` is now
  `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS`. The old names are not read, so a service
  that still sets one gets the default: `Defaults::body_limit` and 30 seconds. The
  Rust constant `config::DEFAULT_HEADER_READ_TIMEOUT_SECONDS` keeps its name.
- **`HTTP_CSP` is now `HTTP_CONTENT_SECURITY_POLICY`.** The old name is not read, so
  a service that set it to allow its pages' scripts or styles gets production's
  strict `default-src 'none'; frame-ancestors 'none'` instead, and its pages break.
  One that set it empty to send no policy now sends the strict one.

### Removed

- **`APP_ENV` is the only variable that names the environment.** `axa` and
  `axa-environment` no longer read Loco's environment variable as a fallback, and
  `axa_environment::VARIABLES` is now `["APP_ENV"]`. A service that still sets only
  the old variable runs as development in a debug build and production in a release
  build, so set `APP_ENV` in every Dockerfile, deploy config, and test script, and
  search your code for any guard that reads the old name itself.

### Fixed

- The Rust examples in the background jobs and idempotency keys guides, and in the
  `axa` skill, are now compiled as doctests, so they cannot drift from the API. CI
  gains a step, `cargo test --doc --features idempotency,paseto`, for the ones that
  need those features. Two examples stay unchecked, as they need `axa-test` or a
  database feature. The skill no longer calls idempotency keys unreleased.

## [0.10.0] - 2026-11-09

This covers `axa` 0.10.0 and fourteen crates new to crates.io, thirteen renamed and one new:
`axa-accepts-version-negotiation` 0.2.0, `axa-problem-details-for-http-apis` 0.1.0,
`axa-request-rate-limit` 0.2.0, `axa-idempotency-key` 0.2.1, `axa-postgres-job-queue`
0.2.0, `axa-transactional-outbox` 0.2.0, `axa-redis-cache` 0.1.1, `axa-web-session`
0.1.0, `axa-tera-templates` 0.2.1, `axa-prometheus-metrics` 0.1.0, `axa-openapi-docs`
0.1.0, `axa-keycloak-auth` 0.3.0, `axa-paseto-auth` 0.5.0, and the new `axa-test`
0.1.0. Each renamed crate is the code of the old crate's last version under its new
name. It breaks 0.9 in one place: the `testing` feature is now `axa-test`.

### Added

- `axa-test`, a new crate: the request-level test helpers, built on `axum-test`, as
  a dev-dependency of their own (spec TST-1, TST-2).

### Changed

- **Breaking: the `testing` feature is gone.** Its helpers are now the `axa-test`
  crate. A service drops `"testing"` from `axa`'s features, adds `axa-test = "0.1"`
  under `[dev-dependencies]`, and changes `axa::testing::prelude` to
  `axa_test::prelude` and `axa::TestServer` to `axa_test::TestServer`. The prelude
  holds the same names, `request`, `boot_test`, `TestApp`, `BootResult`, and
  `TestServer`.

  `axa::prelude` no longer brings them in. With `testing` on, it re-exported them, so
  a test file that imports only `use axa::prelude::*;` and calls `request`,
  `boot_test`, or `TestServer` now fails with E0425 or E0433. Add
  `use axa_test::prelude::*;` to each such file.

- The README no longer says which features no service uses yet, and spec P-2 and
  `AGENTS.md` no longer ask for that mark. P-2 itself stands: two consumers, or one
  with a safety case.
- The README, which crates.io shows, opens with one sentence saying what `axa`
  provides.
- The README's Installing section lists the crates as bullets, each led by its
  crate name, in place of the feature table.
- **Renamed crates**, so each name says what it does:

  | Old | New |
  |---|---|
  | `axa-versioning` | `axa-accepts-version-negotiation` |
  | `axa-problem` | `axa-problem-details-for-http-apis` |
  | `axa-ratelimit` | `axa-request-rate-limit` |
  | `axa-idempotency` | `axa-idempotency-key` |
  | `axa-worker` | `axa-postgres-job-queue` |
  | `axa-outbox` | `axa-transactional-outbox` |
  | `axa-redis` | `axa-redis-cache` |
  | `axa-session` | `axa-web-session` |
  | `axa-tera` | `axa-tera-templates` |
  | `axa-prometheus` | `axa-prometheus-metrics` |
  | `axa-openapi` | `axa-openapi-docs` |
  | `axa-keycloak` | `axa-keycloak-auth` |
  | `axa-paseto` | `axa-paseto-auth` |

  A service that depends on `axa` changes nothing for these: every feature and
  every `axa::` module keeps its name, and so does every table, such as
  `axa_queue`, `axa_outbox`, and `axa_idempotency`, so no migration is needed. A
  service that depends on one of these crates directly changes the name in its
  `Cargo.toml` and its `use` paths. The old names stay on crates.io at their last
  versions.

### Fixed

- `axa-opentelemetry`: a test now completes a TLS handshake with a collector whose
  root the system trusts, and is refused by one whose root it does not (spec OTL-4).
  It makes its roots with `rcgen`, a new dev-dependency, which ships in nothing.
- `axa-environment`: CI runs its tests in a release build too, proving a release
  binary given no name is production (spec ENV-4).
- Spec CLI-9 now says what `axa` owns: `check` returns the refusal, and a `main` that
  hands it on exits non-zero.
- `axa-redis`: a Redis test now proves a failed read, from a lost connection or a
  stalled Redis, is logged without its key (spec RDS-7). No spec row is left
  unverified.
- `axa-idempotency`: two Postgres tests that lose a run no longer drop its request
  after a set 100 ms. On a slow machine the run had not started, so the retry became
  the run that hangs, and CI hung for three hours. They now drop it once it has
  started. CI's `check` job stops after 30 minutes.
- CI's `cargo audit` ignores RUSTSEC-2026-0459 (C SQLite in `libsqlite3-sys` 0.37)
  until sqlx releases the raised cap on it. Nothing in `axa` builds SQLite, and
  `cargo deny` still checks what is built.

## [0.9.1] - 2026-11-08

This covers `axa` 0.9.1 and `axa-idempotency` 0.2.1. Nothing breaks, and a service has
nothing to change. A client polling a run still going is told to wait longer.

### Changed

- `axa-idempotency`: a duplicate of a run still going is told to wait as long as the
  first has run so far, rounded up, from 1 second to the end of its lease. It was
  always told 1 second, so a client that obeyed asked about a lost run once a second
  for up to 24 hours. Now it asks some 17 times.

### Fixed

- `axa-redis`: tests now prove that a log line names neither a key nor a stored value,
  for an unreadable entry and for a failed cache write (spec RDS-7).

## [0.9.0] - 2026-11-08

This covers `axa` 0.9.0, `axa-idempotency` 0.2.0, and `axa-worker` 0.2.0. All three
break; see [Upgrading to 0.9](help/upgrading-to-0.3/index.md#upgrading-to-09).
`axa-offset-pagination` leaves the workspace, and is not published again.

### Removed

- **Offset paging, with `axa-offset-pagination`, breaking (P-10).** `axa` has no
  `offset-pagination` or `pagination` feature, and no `axa::offset_pagination` or
  `axa::pagination`. Its total could count rows a record policy hides. Page with
  `cursor-pagination`. The crate stays on crates.io for Axa 0.8 and earlier, and is not
  published again. See
  [Upgrading to 0.9](help/upgrading-to-0.3/index.md#upgrading-to-09).

### Changed

- **`axa-idempotency` 0.2.0 holds a lost run's key for its whole `ttl`, and breaks.** A
  run lost to a crash, a hang-up, a panic, or a timeout may have done its work. Its
  claim used to lapse after a minute, and a retry of the same request then ran the
  write again. Now the lease is the key's `ttl` by default, so a retry gets the `409`
  `idempotency_in_progress` until the key expires, and the write never runs twice under
  one key (P-10).
  - `lease` is gone. `rerun_abandoned_after(lease)` sets a shorter lease, and says in
    its name that it lets the write run a second time.
  - `rerun_abandoned_after` panics at boot under 60 ms or over the `ttl`, and `ttl`
    panics under 60 ms or under a lease already set.
  - A run reports in every third of its lease, so every 8 hours under the default
    24-hour `ttl`.
- **`axa-worker` 0.2.0 keeps a person's acts on jobs as a record, and breaks.** Job rows
  stay state. `cancel` and `retry`, and the release of a job in `needs_review`, now take
  the caller's transaction, a `by`, and a `reason`, and append the act to a new
  insert-only `axa_job_acts` in the same statement as the change, so a service's audit
  row commits with both. Its triggers refuse `UPDATE`, `DELETE`, and `TRUNCATE` from any
  role, and `connect_existing` refuses to start without them. Acts outlive the job's
  row, and `Queue::acts` reads them. The forms that took a pool are gone. See
  [Upgrading axa-worker to 0.2](help/upgrading-to-0.3/index.md#axa-worker-02).

### Fixed

- Two `axa-worker` schedule tests failed on a busy machine. Six replicas finishing every
  job at once could deadlock each other, so each now finishes only rows no other holds.
  The test of due times allowed a job half an interval to be queued, and a slow poller
  took longer. It now allows a whole interval, and the due times are still checked
  exactly.

## [0.8.0] - 2026-11-08

This covers `axa` 0.8.0, `axa-outbox` 0.2.0, and `axa-cursor-pagination` 0.1.1.
`axa-outbox` 0.2.0 breaks; see
[Upgrading to 0.8](help/upgrading-to-0.3/index.md#upgrading-to-08). Nothing else a
service uses changes.

### Added

- A test for LOG-4, the one requirement in the spec with no test at all: logs written
  to a pipe carry no colour.
- A test for CFG-16's last part: `logger::init` writes settings warnings to stderr,
  even with logging off.
- **`outbox-encryption`, and the `encryption` feature of `axa-outbox`:** payloads sealed
  at rest with XChaCha20-Poly1305 under keys the service supplies, bound to their event,
  with a key id so that a key can be replaced. A key's material is wiped from memory
  when it is dropped. It adds `chacha20poly1305` 0.11, and uses `zeroize`, already in
  the workspace through `axa-paseto`.
- `axa::outbox::inbox::seen`, an insert-only inbox a consumer dedupes with in its own
  transaction.

### Changed

- **`axa-outbox` 0.2.0 keeps a record, and breaks.** An event and everything that
  happened to it are only ever inserted, into `axa_outbox_events`,
  `axa_outbox_attempts`, and `axa_outbox_payloads`, and their triggers refuse `UPDATE`,
  `DELETE`, and `TRUNCATE` from any role. An event's state is its latest attempt, and
  every attempt names its relay and its claim. `connect_existing` refuses to start
  without the triggers. See
  [Upgrading axa-outbox to 0.2](help/upgrading-to-0.3/index.md#upgrading-axa-outbox-to-02).
  - `Outbox::run` takes a decision, run before the publisher in the claim's
    transaction, answering `Deliver`, `Refuse`, which is final, or `Hold`, which charges
    nothing. It also takes a record, run in each outcome's transaction, so a service's
    audit row commits with the outcome. Both are required: `decide::deliver_all` and
    `record::nothing` say there is neither.
  - The outcome methods of a service's own loop, `delivered`, `failed`, `refused`, and
    `returned`, take the caller's transaction, and there is no form that takes a pool.
  - Nothing is swept: `retention_days`, `purge_sent`, and `retry_failed` are gone. A
    person puts a failed event back with `release`, and erases a payload with `erase`,
    each naming who and why.
  - A fingerprint is checked before each decision, and a changed event is
    `failed: tampered` and never published.
  - A reason is a code of at most 200 characters with no line breaks. Free error text is
    never stored or logged.
  - `write` refuses a payload over 64 KiB. `Writer` takes other limits and a list of
    denied field names.
  - A topic can be given `Order::None`.
  - `IMPORT_STATEMENTS` copy the 0.1.0 table's rows into the record.
  - The working state is guarded too. `axa_outbox_owed` changes only as the record
    does, apart from a hold of at most a day, and `REBUILD_OWED_STATEMENTS` make it again
    from the record. A lease cannot be dated ahead or moved. `hold_interval` is at most a
    day.
- The spec gains principles P-7 to P-10, and P-2 admits code that one service needs to
  keep a control it is held to.
- `axa-cursor-pagination` uses `base64` 0.23. Cursors are encoded as before, so a
  cursor issued by 0.1.0 still reads.
- The `axa` agent skill explains how to page a collection with signed cursors.

### Fixed

- An `axa-outbox` relay given up on could mark a row sent by its id alone. Every outcome
  now matches the claim it ends.
- Two `axa-outbox` heartbeat tests waited a fixed 100 ms, and could fail on a busy
  machine. They now wait for the claim and for the relay to report in.
- `axa-tracing`'s stalled-runtime test could stop the monitor before it logged, and
  failed about one run in twenty. It now waits for the warning.
- `axa-worker`'s delayed-job test gave its job 400 ms, and on a slow CI runner the job
  came due before the test looked. It now has 2 seconds, and the test waits for it.

## [0.7.0] - 2026-11-07

This covers `axa` 0.7.0, `axa-ratelimit` 0.2.0, and five new crates: `axa-worker`
0.1.0, `axa-idempotency` 0.1.0, `axa-outbox` 0.1.0, `axa-cursor-pagination` 0.1.0, and
`axa-offset-pagination` 0.1.0. `axa-offset-pagination` supersedes `axa-pagination`
0.1.1. Some changes break; see
[Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md), whose section on 0.7 says what
to change.

### Added

- **`axa-worker`, the queue's engine, now its own crate.** The `queue` feature uses it,
  and `axa::queue` keeps its paths and signatures. New in it:
  - A limit per job type in a process, `Queue::limit`, below `QueueOptions::workers`.
  - Delayed jobs, `run_in` and `run_at`, on the database's clock, including in a
    transaction with `queue::enqueue_job_on`, and `BackgroundWorker::perform_in`.
  - Priorities, and batches queued whole or not at all with `Queue::enqueue_all`.
  - A job's status by id with `Queue::job`, and `cancel`, `retry`, and `counts`.
  - A time limit only when one is set: `NewJob::timeout` or `BackgroundWorker::timeout`.
  - **A run that lost its claim is stopped**, and its job waits in `needs_review` for a
    person, instead of finishing or being retried by itself.
  - **Schedules**: `Queue::every` queues a job each interval, once per due time across
    replicas, with no drift, one job for missed times, and no stacking. `first_at` sets
    the first time. A schedule removed from code is archived, never deleted, and
    restored if it returns. `Queue::schedules` lists them.

- **`axa-ratelimit` 0.2.0, all opt-in, with today's behaviour unchanged by default:**
  - `Limiter::key` and the `by_key` layer count by a key a service takes from the
    request, such as the signed-in user. A request with no key is not counted.
  - `WhenFull` says what a key not yet held gets once `MAX_KEYS` are: `Share`, as
    before and still the default, `Allow`, or `Refuse`. `Verdict::overflowed` says
    when it decided.
  - `Verdict::first_refusal` is true once per key per window, so a service can write
    one audit row for a client that went over.
  - Every response from `by_client`, `by_key`, and `lock_out_failures` carries its
    `Verdict` as a response extension, for the service's own audit or metric layer.
  - `Limiter::set_limit_for` and `clear_limit_for` change one key's limit from its next
    request, with no restart. At most `MAX_OVERRIDES` are held, and past that setting
    another is refused, never making room by dropping one.
  - `by_client` warns once, without the address, when its first 100 requests all came
    from one private or loopback address, as they do from a proxy `TRUSTED_PROXIES`
    does not list. `axa-ratelimit` now depends on `tracing`, which `axa` already did.

- **Conditional requests, in `axa::conditional`.** The standard stack now answers `304` to
  a `GET` or `HEAD` holding the `ETag` of its answer, whoever set the tag, with weak
  comparison, lists, and `*` as RFC 9110 has them. A `304` keeps what the `200` would have
  said, such as `vary` and `cache-control`, and it gets the request id, security headers,
  and CORS headers. It is made from the answer the caller would have had, so a refused
  caller gets the refusal. PPM, CRM, and patient-flow had each written their own.
- `conditional::etag`, a layer a route or router opts into, gives a `GET`'s `200` a weak
  tag from a hash of its body. It reads only a body of exact known size up to
  `conditional::MAX_BODY` (8 MB), so streams and server-sent events pass through unread,
  and it skips `no-store`. No new dependency: the hash is std's.
- `ETag`, which goes on a response like any header: `ETag::weak` and `ETag::strong` from
  text, `ETag::weak_of` and `ETag::strong_of` from a value's JSON.
- `conditional::if_match`, `require_if_match`, and `if_match_holds` for a stale write,
  which is a `412` problem with the `code` `precondition_failed`, and a missing `If-Match`
  a `428` with `precondition_required`.

- **`axa-idempotency`, behind the `idempotency` feature:** `Idempotency-Key` handling
  over Postgres, so a retried write gets its first answer back, marked
  `Idempotent-Replayed: true`, instead of running twice. It is one layer, which a service
  adds inside its guards and inside `axa::middleware::apply`. A closure names the caller,
  and a key is per caller. The same key on a different request is a `422`, a duplicate
  while the first runs is a `409`, and a store that cannot be reached is a `503` before
  anything runs. A `4xx` or `5xx` frees the key. A run reports in while it works, so a
  crash frees a key in about a minute and a long handler is never run twice, and a run
  that lost its key cannot overwrite the next run's answer. Three services and the
  bulk jobs of four more had written their own. No service uses this yet.
  [Where the layer goes](help/idempotency-keys/index.md).
  `Idempotency::required` refuses a write with no key on the routes it names.
- With the `idempotency` feature, `Error` converts from `sqlx::Error`, so `?` works on
  `Idempotency::check` in a service on SeaORM.

- **`axa-outbox`, behind the `outbox` feature:** a transactional outbox and relay on
  Postgres. `outbox::write` puts an event in the caller's transaction, sqlx or SeaORM,
  and `Outbox::run` publishes it through a closure the service passes in, so the broker
  client stays in the service. It is at least once, keeps each key in order, charges
  a broker outage to no row, leaves a row it cannot publish as `failed` without holding
  up other keys, and brings back a claim whose relay died. A service with a batching
  client can run its own loop on `claim` and `sent`. Thirteen services had written
  their own outbox table, and ten their own relay. See
  [The transactional outbox](help/transactional-outbox/index.md). The `outbox` feature
  turns on `queue`.

- `SEND_STALL_TIMEOUT_SECONDS`, 60 by default, drops a client that has stopped reading a
  response. **Breaking:** `config::Server` gains `send_stall_timeout_seconds`, so code
  building one by hand must set it.

- **`axa-cursor-pagination`, behind the `cursor-pagination` feature:** signed cursor
  pages. A list takes `?limit=` and an opaque `?cursor=`, and answers a bare JSON array
  with `Link: rel="next"` while more rows follow, and no total, because a total would
  count rows the caller's record policy hides. A cursor is signed with HMAC-SHA256 under
  the service's `CursorKey`, over its position and its query, so a caller cannot make
  one up, edit one, or carry one to another list or filter. A bad limit or cursor is a
  `422`, never clamped, and converts to `axa::Error`. CRM pages this way (CRM-T51), with
  an unsigned cursor. It adds `hmac`, `sha2`, and `base64`, which `axa` already pulled
  in through other crates.

### Changed

- **The queue's tables change.** `axa_queue` gains `priority` and `timeout_ms`, and
  there is a new `axa_schedule`. `Queue::connect` adds them. A service on
  `Queue::connect_existing` must run the new `queue::SETUP_STATEMENTS`, which now has
  eight statements, or it stops at boot.
- **A run that finishes after its claim was taken now marks its job `needs_review`**,
  where it only logged before.
- Cancelled rows are deleted with finished ones after `retention_days`.
- A job's error text kept on its row is cut to 1024 bytes.
- `Queue::run` stops its jobs when it is dropped, so a hung job no longer outlives the
  shutdown time limit in the background.
- The queue's pool has `workers + 2` connections, one more than before, for schedules.

- **`axa-pagination` is now `axa-offset-pagination`**, with the same API, and the
  published `axa-pagination` 0.1.1 is superseded. Its feature is `offset-pagination`,
  as `axa::offset_pagination`. The `pagination` feature and `axa::pagination` path stay
  as its old names, and will go. Its documentation now says plainly that offset paging
  is insecure, and points to `axa-cursor-pagination`.
- With origins listed, CORS exposes `etag`, so a page can read a tag to send back.

### Fixed

- A client that stopped reading a streaming response held its connection slot for ever,
  and could hold up shutdown until its limit. The keep-alive limit only covers a response
  whose body has ended, so now the send stall limit covers both.

## [0.6.2] - 2026-11-07

This covers `axa` 0.6.2. No subcrate changes. A write that breaks a check or not-null
constraint answers `422`, not `500`; see
[Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md), whose section on 0.6.2 says what
to check.

### Fixed

- **A violated check or not-null constraint answered `500` and logged an error.** The
  write was the caller's mistake, as a unique or foreign key violation is. It now
  answers `422` with code `unprocessable_entity`, and logs as a client's mistake. Its
  detail names no column or value: "A value is outside what this field allows" or "A
  required value is missing". This holds through sqlx, `Error::DB`, and
  `ModelError::DbErr`.

## [0.6.1] - 2026-11-07

This covers `axa` 0.6.1. No subcrate changes. A rejected JSON body's `code` changes at
`413`, `415`, and `422`; see
[Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md), whose section on 0.6.1 says what
to check.

### Fixed

- **The keep-alive limit cut off a response still reaching a slow client.** `hyper`
  drops a response's body once it holds the last of it, so a body in one piece, such as
  JSON or a `Vec`, started the idle timer as sending began. A response taking over 75
  seconds to deliver was truncated, and at shutdown too. A connection now counts as idle
  from the last byte written.
- An upgraded connection released its slot, so WebSockets did not count towards
  `MAX_CONNECTIONS`. It now keeps the slot until its socket closes.
- A constraint violation through the model layer, as `ModelError::DbErr`, answered `500`
  and logged an error. It now answers `409`, as `Error::DB` does.
- `axa::extract::Path` blamed the caller with `bad_request` on a route that does not fit
  it. It now answers the generic `500`.
- A rejected JSON body said `code: bad_request` at every status. A `413`, `415`, or `422`
  now says `payload_too_large`, `unsupported_media_type`, or `unprocessable_entity`.
- A timeout too long for the clock, such as `18446744073709551615`, panicked the accept
  loop at the first connection. It now means no limit.

## [0.6.0] - 2026-11-07

This covers `axa` 0.6.0, `axa-keycloak` 0.3.0, `axa-pagination` 0.1.1, `axa-paseto` 0.5.0,
`axa-ratelimit` 0.1.0, `axa-tera` 0.2.1, and `axa-versioning` 0.2.0. It breaks 0.5: every
error's body changes, and `config::Server` has a new field. See
[Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md), whose section on 0.6 lists what a
service changes.

### Added

- **`axa-ratelimit`, behind the `ratelimit` feature:** a limit per client in a fixed
  window, and a lockout that counts only `401`s, with the `429`, `Retry-After`, and
  `RateLimit-*` headers. It keys on the address `axa` resolved, so a forged
  `X-Forwarded-For` buys no fresh limit. Four projects had written their own.
- `KEEP_ALIVE_TIMEOUT_SECONDS`, 75 by default, sets how long an idle connection is kept.
  **Breaking:** `config::Server` gains `keep_alive_timeout_seconds`, so code building
  one by hand must set it.
- `axa::extract`, with `Json`, `Path`, and `Query` that reject with a problem saying what
  was wrong and quoting nothing the caller sent. The prelude keeps Axum's, whose
  rejections the standard stack makes problems with no `detail`, so a service opts in by
  importing these.
- `Task::development_only`, which refuses a task outside development and test, for a
  task that loads fixtures. `task seed` could put made-up records into production.
- In `axa-pagination` 0.1.1: `Page::bounds` and `Page::respond`. With the `pagination`
  feature, an `OffsetTooLarge` converts to `Error::BadRequest`, so `?` works on
  `resolve`.

### Changed

- `Cli::parse` and `Cli::try_parse` now refuse `db down`, `db reset`, and `db truncate`
  as `cli::parse` does, so a service cannot miss the guard.

- **Breaking: errors are RFC 9457 problems.** Every error `axa` answers is
  `application/problem+json`, with `type`, `title`, `status`, and where there is more to
  say `detail`. Successes stay bare JSON. The body used to be `{error, description,
  errors}`: `error` is now the `code` extension member, `description` is `detail`, and
  `errors` is an extension member as before. See
  [Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md), whose section on 0.6 lists what
  a service changes.
- **Breaking: `ErrorDetail`'s fields are `code` and `detail`**, in place of `error` and
  `description`. `ErrorDetail::new` and `with_reason` take what they took.
- **Breaking: some `code`s differ from the old `error`.** `Error::BadRequest` and a
  rejected JSON body say `bad_request`, not `Bad Request`, a model's validation failure
  says `validation`, where it said nothing, and a `500` has no `detail`.
- **Breaking: `axa-paseto` and `axa-keycloak`'s `Rejection` is a struct**, not a
  `(StatusCode, String)` alias, and answers as a problem rather than plain text.
  `rejection.0` and `rejection.1` still work. `axa-keycloak` writes the body itself, so
  that it still depends on no `axa` crate.
- **Breaking: `axa-versioning`'s `406` is a problem**, with `code` in place of `error`,
  and `requested` and `supported` as extension members.
- **Breaking: the standard stack makes a bare refusal a problem.** An error with no
  `content-type`, or with `text/plain`, becomes a problem for its status with no
  `detail`, and keeps its headers. That covers Axum's unmatched path, wrong method, and
  extractor rejections, whose text can quote the request. A handler's own plain-text
  error is dropped the same way, so it should return an `Error`.
- `axa-tera` answers a failed render as `text/html`, so that a browser still gets text
  and the stack leaves it alone.
- A request past its time limit is a `408` problem, where it had an empty body.
- A failing `/_readiness` is a `503` problem, which still carries `ok` and `failed`.
- `axa` always depends on `axa-problem`, and `axa::problem` always exists. The `problem`
  feature adds nothing, and stays so that a service naming it still builds. `axa-paseto`
  and `axa-versioning` now depend on `axa-problem` too. No external dependency is added.

### Fixed

- The Loco guide told services to call `into_app` and then add their own layers, which
  puts them outside the standard stack, so an auth guard's refusal had no request id,
  security headers, or CORS headers. It now says to add them first and call
  `middleware::apply` last, and the README, the skill, and the upgrade guide say so too.
- The README and spec said no service uses `axa-problem`, and several do.
- The README now marks every subcrate no service uses, as the spec requires. Five were unmarked.
- A panic's `500` gets the security and CORS headers. The panic was caught outside them.
- An idle connection no longer closes at the header limit. `hyper` runs one timer
  across both, so a load balancer that keeps connections for longer could reuse one as
  it closed, and answer `502`. A `Tracker` in `server` now runs the two limits.
- A violated unique or foreign key constraint now answers `409`, not `500`, and logs
  as a client's mistake rather than an error.
- A rejected JSON body now says what was wrong, in `detail`. It used to answer with only
  a status and a reason.

## [0.5.0] - 2026-11-06

This covers `axa` 0.5.0, `axa-diesel` 0.2.0, `axa-keycloak` 0.2.0, `axa-opentelemetry`
0.1.1, `axa-paseto` 0.4.0, `axa-redis` 0.1.1, `axa-seaorm` 0.2.1, `axa-sqlx` 0.1.2, and `axa-versioning` 0.1.1. It breaks 0.4 for
code that builds `config::Http`, `config::Server`, or `axa_diesel::PoolConfig` without
`..`, because each has a new field. See
[Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md), whose section on 0.5 also shows
how to add `axa-tracing` and `axa-opentelemetry`.

### Added

- **`axa::client`: the real client address.** `X-Forwarded-For` is believed only from
  proxies listed in `TRUSTED_PROXIES`, read from the right, so an entry the caller wrote
  is not. A handler takes `ClientAddr`, and a layer reads a plain `IpAddr` request
  extension. The address stays out of the request span, because it is personal data.
- **Every refusal writes an audit line.** `axa-paseto` and `axa-keycloak` log
  `auth_refused` at `warn`, with status, action, client address, and for a policy
  refusal the subject's id and reason. It never holds the token or email. It replaces
  `paseto_forbidden` and `keycloak_forbidden`.
- `MAX_CONNECTIONS`, the setting for `server::MAX_CONNECTIONS`, which is its default.
- In `axa-keycloak` and `axa-paseto`: `Guard::authorize`, `Guard::honouring`, and the
  `Allowed` extractor, which gives a handler the decision. `AuthUser` no longer verifies
  a token the middleware has already verified.
- `KeycloakVerifier::ready`, for `/_readiness`.
- `axa_versioning::ApiVersions`, so that a service's 2.0 needs no `axa` release.
- `axa_diesel::PoolConfig::wait_timeout`. A pool that was all in use made a caller wait
  for ever.

### Fixed

- `axa-opentelemetry` counts a panic and a cancelled request, with `error.type`. They
  used to leave no metric, as they never reached the recording line.
- `axa-redis` logs neither keys nor error text that can quote a stored value.
- The `Debug` text of `PoolConfig`, in `axa-sqlx` and `axa-seaorm`, no longer prints the
  connection string, which can carry a password.
- The examples compile under the `sqlx` feature, and CI now builds it without `sea-orm`.

### Changed

- **`axa-keycloak` and `axa-paseto` refuse a decision with an obligation the service has
  not declared.** They used to allow the request and drop the condition, so a rule asking
  for `"audit"` got none. Declare what a service carries out with `Guard::honouring`.

## [0.4.1] - 2026-11-06

This covers `axa` 0.4.1. Its API has no breaking change, and two behaviours are
stricter: `cli::parse` refuses commands that destroy data, and a server that had HTTP/2
turned on by another dependency now speaks HTTP/1 only.

### Added

- `DbCommand::destroys_data` and `cli::DESTROY_DATA_VAR`.
- Two AI agent skills: `skill/axa-skill` for building a service with Axa, and
  `skill/axa-maintainer-skill` for working on Axa itself.

### Fixed

- **The header limit now covers a client that sends nothing.** `HEADER_READ_TIMEOUT_SECONDS`
  applied only once a client had sent its first bytes, because protocol detection ran
  first with no limit, so a silent connection stayed open for ever. The server now
  speaks HTTP/1 directly. It was only ever built for HTTP/1, so no service loses
  HTTP/2, unless another dependency turned it on, and then a proxy can offer it.
- **A server holds at most `server::MAX_CONNECTIONS`, 10,000, connections.** More wait
  to be accepted. It becomes a setting in the next minor release.
- **`hostaddr` no longer gets past the plaintext check.** `postgres://localhost/app?hostaddr=10.0.0.5`
  counted as loopback, and the driver connected to `10.0.0.5`.
- The `Debug` text of `Database`, `Config`, and `AppContext` no longer prints the
  database password.
- The warning that no worker serves the queue no longer holds a stray backslash.

### Changed

- `cli::parse` now refuses `db down`, `db reset`, and `db truncate` unless the process
  runs in development or test and `AXA_DESTROY_DATA` is `true`. It exits before the
  service runs anything, and `--environment` cannot lift the refusal. A hand-written
  `run` could leave the guard out, and one did. `Cli::parse` does not refuse them.

## [0.4.0] - 2026-11-06

This covers `axa` 0.4.0, `axa-paseto` 0.3.0, `axa-seaorm` 0.2.0, `axa-sqlx` 0.1.1,
`axa-environment` 0.1.1, and the first releases of `axa-keycloak`, `axa-tracing`, and
`axa-opentelemetry`, each 0.1.0. It breaks 0.3.0 where noted: `Defaults` has new fields, and the default database
timeouts are longer. See [Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md), whose
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
see [Upgrading to Axa 0.3](help/upgrading-to-0.3/index.md).

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
  It reads the settings Axa knows about and connects to nothing, so it says
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
