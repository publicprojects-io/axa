# Loco

Loco is a Rails-like web framework for Rust, built on Axum and SeaORM. Its repository
is <https://github.com/loco-rs/loco>. I read commit
[`0fce092`](https://github.com/loco-rs/loco/tree/0fce092950d4989b9d4add2c70439cf59ee47557)
of 6 November 2026, when the crate was `loco-rs` 1.3.1. The repository has about 9,600
stars and about 79,000 lines of Rust, counting the `loco-gen` generator and its tests.
Axa was taken from what the Main X Index services used in Loco, so this comparison
looks mostly at what Loco has that Axa does not. Loco drives the application through a
`Hooks` trait, builds an `axum::Router` from `AppRoutes`, and applies a named
middleware stack that the application edits by name.

## How it is built

Links point at commit `0fce092`.

| Area | What Loco does | Source |
|---|---|---|
| Control flow | A `Hooks` trait with about twenty methods: `boot`, `routes`, `after_context`, `before_routes`, `after_routes`, `initializers`, `middlewares`, `connect_workers`, `register_tasks`, `truncate`, `seed`, `dump`, `on_shutdown`, and more. The framework calls them | [`src/app.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/app.rs) |
| App state | `AppContext` holds the database, queue, mailer, storage, cache, and a type-keyed `SharedStore`. An `AppContextBuilder` lets `after_context` swap any of them | [`src/app.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/app.rs) |
| Initializers | A trait with `before_run`, `after_routes`, and `check`. The one built in, `MultiDbInitializer`, reads its settings from an untyped `initializers` map in the YAML and adds an `Extension` | [`src/app.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/app.rs), [`src/initializers/multi_db.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/initializers/multi_db.rs) |
| Middleware | A `Vec<Box<dyn MiddlewareLayer>>`, each named and configured from YAML: limit payload, CORS, catch panic, ETag, remote IP, compression, timeout, static assets, secure headers, logger, request id, fallback, powered-by, auth session. `insert_before`, `insert_after`, `replace`, and `delete` edit it by name | [`src/controller/middleware/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/controller/middleware/mod.rs) |
| Configuration | One YAML file per environment, rendered through Tera first, so `get_env` can read variables | [`src/config/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/config/mod.rs) |
| Mailer | A `Mailer` trait. `mail` queues the message on a `MailerWorker`, and `deliver_now` sends inline. Templates are Tera directories embedded with `include_dir`, with inheritance and shared layouts. SMTP through `lettre`, or a stub transport whose `deliveries()` a test reads | [`src/mailer/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/mailer/mod.rs), [`src/mailer/template.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/mailer/template.rs), [`src/mailer/email_sender.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/mailer/email_sender.rs) |
| Storage | A `StoreDriver` trait with upload, get, delete, rename, copy, exists, list, stat, streaming reads and writes, and presigned GET and PUT. Drivers: local disk, memory, null, and S3, Azure, and GCS through OpenDAL. A `ReplicatedStrategy` mirrors writes to several stores or keeps a backup | [`src/storage/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/storage/mod.rs), [`src/storage/drivers/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/storage/drivers/mod.rs), [`src/storage/strategies/replicated.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/storage/strategies/replicated.rs) |
| Cache | A `Cache` over drivers for memory, Redis, the database, and null. `insert` stores with no expiry, and `get_or_insert` loads on a miss | [`src/cache/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/cache/mod.rs), [`src/cache/drivers/`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/cache/drivers) |
| Notifications | A `Notifier` trait that renders one message per channel (Slack, signed JSON webhook, email) and queues each channel as its own job, so one failing channel does not resend the others. Webhooks carry an HMAC-SHA256 signature over `timestamp.body`, with `verify_signature` for receivers | [`src/notifications/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/notifications/mod.rs), [`src/notifications/webhook.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/notifications/webhook.rs) |
| Auth | HS256/384/512 JWTs, argon2 password hashing, a signed or encrypted cookie session with `CurrentUser` extractors, server-side session stores in the database or Redis, and OAuth2 sign-in routes with an `OAuthHandler` trait | [`src/auth/`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/auth), [`src/auth/session.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/auth/session.rs), [`src/auth/oauth/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/auth/oauth/mod.rs), [`src/hash.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/hash.rs) |
| Validation | `JsonValidate`, `FormValidate`, and `WithMessage` extractors over `validator` | [`src/controller/extractor/validate.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/controller/extractor/validate.rs) |
| Background work | Queues on Postgres, SQLite, and Redis, and a separate cron scheduler that reads cron or English phrases from YAML and runs tasks or shell commands | [`src/bgworker/`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/bgworker), [`src/scheduler.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/scheduler.rs) |
| Multi-tenancy | Opt-in `TenantEntity` trait for SeaORM, with `in_tenant` on queries and `set_tenant` on active models | [`src/model/tenant.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/model/tenant.rs) |
| Seeding | `seed` and `dump` move YAML fixtures in and out of tables, and reset sequences | [`src/db/seed.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/db/seed.rs) |
| Health | `/_ping`, `/_health`, and `/_readiness` | [`src/controller/monitoring.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/controller/monitoring.rs) |
| Static files | `StaticAssets` middleware with a fallback file and precompressed files, or assets embedded in the binary | [`src/controller/middleware/static_assets.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/controller/middleware/static_assets.rs) |
| Scaffolding | `cargo loco generate` for model, migration, controller, scaffold (API, or HTML pages), mailer, worker, task, scheduler, notifier, OAuth controller, data, and Docker, nginx, and Lambda deployment files | [`loco-gen/src/lib.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/loco-gen/src/lib.rs), [`loco-gen/src/templates/`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/loco-gen/src/templates) |
| Test helpers | `request` and `request_with_create_db`, which makes a fresh Postgres database or SQLite file for each test and drops it after. `seed` loads fixtures. `RequestConfig` keeps cookies and sets a default content type. Redaction filters for snapshot tests, and CSS selector assertions over HTML (`assert_css_eq`, `assert_link`, `assert_count`) | [`src/testing/request.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/testing/request.rs), [`src/testing/db.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/testing/db.rs), [`src/testing/redaction.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/testing/redaction.rs), [`src/testing/selector.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/testing/selector.rs) |

## Compared with Axa

| Area | Loco | Axa |
|---|---|---|
| Control flow | `Hooks` trait the framework drives | `main.rs` calls plain functions. Hooks are on Axa's "must not be added" list |
| Configuration | YAML per environment, rendered by Tera | Environment variables with defaults in code, and production refuses unsafe or invalid values |
| Middleware | Named, editable stack, configured in YAML. Powered-by header by default | Fixed stack in a documented order. No powered-by header. Security headers everywhere, HSTS in production |
| Errors | `loco_rs::Error` to JSON | RFC 9457 problems, internal text never sent, constraint violations to `409` and `422` |
| Health | `/_ping`, `/_health`, `/_readiness` | `/_health/live`, `/_health/ready`, `/_health/startup`, with named checks, shared answers, and time limits |
| Background jobs | Postgres, SQLite, Redis queues | Postgres, MySQL, SQLite queues with heartbeats, audited cancel and retry, `needs_review`, and schedules once per due time across replicas |
| Schedules | Cron and English phrases, run as tasks or shell commands | `Queue::every` with a fixed interval. No cron or calendar times |
| Cache | Memory, Redis, database. Entries may have no expiry | Redis or memory. Every entry expires. A Redis failure falls back to the loader |
| Sessions and auth | JWT, cookie session, server-side session, OAuth2 sign-in, argon2 | PASETO and Keycloak bearer auth with ABAC, web sessions with form tokens. No sign-in flow, no password hashing |
| Mailer | `Mailer` trait, queued, Tera templates, stub transport for tests | None. The Loco guide says to use `lettre` directly |
| Storage | `StoreDriver` with local, memory, S3, Azure, GCS, presigning, replication | None |
| Notifications and webhooks | Slack, signed webhooks, email, each channel retried alone | The outbox records and relays events, and the service brings its own publisher |
| Validation | Validating extractors | `Validatable` and `validation`, called by hand. Problem-shaped `Json`, `Path`, `Query` extractors |
| Multi-tenancy | SeaORM tenant scoping | None |
| Seeding | YAML fixtures, seed and dump | None |
| Static files | Middleware, fallback page, embedded assets | None. A service uses `tower-http` `ServeDir` |
| Scaffolding | `cargo loco generate`, many generators | None. Code generation is on the "must not be added" list |
| Tests | Per-test database, fixtures, cookies, snapshot redaction, HTML selectors, mail deliveries | `axa-test`: `boot_test` and `request` over `axum-test`. Database helpers `migrate`, `truncate`, and `dangerously_recreate` |
| Observability | `tracing` logger | Request span with route template, Prometheus, OpenTelemetry, redaction |
| Other | | Idempotency keys, outbox, cursor pagination, version negotiation, conditional requests, OpenAPI drift checks, connection-level timeouts |

## What Axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| A test database per test | Tests that share one database must run one at a time or truncate between them. A fresh database per test lets them run in parallel | M | High |
| Captured mail and log assertions in `axa-test` | A test needs to see that a mail would have gone out, and what it said, without a mail server | S | Medium |
| A small mail crate over `lettre` | Every service that sends sign-in links or receipts writes the same SMTP setup, TLS rules, and test transport. The Loco guide already names the timing hazard | M | Medium |
| An object storage crate | Uploads and exports need local disk in development and S3 or GCS in production behind one API, with presigned URLs | M | Medium |
| Cron and calendar schedules | `Queue::every` covers "every hour", but not "at 02:00 on weekdays" or a time zone | M | Medium |
| A signed webhook publisher and verifier | A common outbox publisher. The signing scheme (HMAC over timestamp and body) is generic | S | Medium |
| Fixtures for tests | Loading known rows before a test is common, though each service has its own models | S | Low |
| A validating extractor | `Json` that calls `Validatable` and answers `422` with field errors, so handlers do not forget | S | Low |
| HTML selector assertions | Useful for services that render Tera pages | S | Low |
| OAuth2 sign-in routes | Useful only for services that sign people in themselves, not behind Keycloak | L | Low |

**A test database per test.** This belongs in `axa-test`. A function such as
`axa_test::db::fresh(&base_url) -> FreshDb` would create a database named for the test
and a random suffix, migrate it with the database crate's own `migrate`, and drop it
when the guard is dropped, as Loco's `PostgresTest` does in
[`src/testing/db.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/testing/db.rs). The test then sets `DB_URL` through
`Config::from_lookup` rather than the process environment, so tests stay parallel. The
drop must refuse any database not made by the guard, and must never run against a
database that `APP_ENV` says is production, in line with `AXA_DESTROY_DATA`. It needs
only what the database crates already depend on.

**Captured mail and log assertions.** If Axa gains a mail crate, it should have a
memory transport whose messages a test reads, as Loco's `EmailSender::stub` and
`deliveries()` do. Without a mail crate, `axa-test` can still offer a log capture: a
`tracing` layer that records events at a level for the current test, so a test can
check that a refusal was logged and that a secret was not. It depends only on
`tracing-subscriber`, which Axa already uses.

**A mail crate.** A crate such as `axa-smtp-mail` would hold a `Mailer` struct built
from environment variables (`SMTP_URL`, `SMTP_FROM`), refuse plaintext SMTP in
production unless a variable allows it (P-3), and offer `send` and a memory transport.
It should not queue by itself. The service chooses inline, `tokio::spawn`, or a job,
because the Loco guide in `help/from-loco-into-axa` shows each choice has a cost: a
queued sign-in link sits in plain text in the job table, and inline sending leaks
whether an account exists. Templates stay with `axa-tera-templates`. It depends on
`lettre` with rustls, which is a new dependency and must be raised first (P-6).

**An object storage crate.** A crate such as `axa-object-storage` would offer a
`Store` with `put`, `get` as a stream, `delete`, `exists`, `list`, and `presign_get`
and `presign_put`, over local disk and memory, and S3 or GCS behind features. Loco
uses OpenDAL; the `object_store` crate is a smaller alternative. Two things should be
stricter than in Loco. Loco's `normalize` in
[`src/storage/drivers/mod.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/storage/drivers/mod.rs) drops empty segments but
not `..`, so the local driver should refuse a key with `..` or an absolute path. I did
not test whether Loco's driver escapes its root. And a presigned URL should require a
time limit, as the cache requires one. Loco's mirror and backup strategies can be left
out until two services need them (P-2).

**Cron and calendar schedules.** This fits `axa-postgres-job-queue` and its MySQL and
SQLite peers as `Queue::at(name, schedule, job)` beside `Queue::every`, where the
schedule is a cron expression and a time zone. It must keep the guarantees `every`
already has: each due time queued once across replicas, one job for times missed, and
removed schedules archived. It would bring in a cron parser and a time zone database,
which must be raised first. Loco's English phrases and shell commands should not come
with it.

**A signed webhook publisher and verifier.** A small crate, or a module of
`axa-transactional-outbox`, with a publisher closure that POSTs an event with a
delivery id equal to `event_id`, a timestamp, and an HMAC-SHA256 signature, and a
`verify` function that checks the signature in constant time and refuses an old
timestamp. This matches Loco's scheme in
[`src/notifications/webhook.rs`](https://github.com/loco-rs/loco/blob/0fce092950d4989b9d4add2c70439cf59ee47557/src/notifications/webhook.rs). It would need an HTTP
client. The client must refuse private and loopback addresses unless allowed, because
the target URL often comes from a user.

## What Axa should not copy

- **The `Hooks` trait and initializers.** Axa's spec forbids lifecycle hooks. A Loco
  initializer is a function that takes and returns a `Router`, and that is what the
  migration guide already says to write. The untyped `initializers` map in YAML is the
  kind of hidden configuration Axa avoids.
- **The editable middleware stack.** Editing layers by name makes the order a matter of
  configuration. Axa keeps one documented order and lets a service add layers before
  `apply`.
- **YAML rendered through Tera.** A template engine in front of configuration is how an
  empty `get_env(default="")` becomes an empty secret, as the Axa migration guide warns.
- **Scaffolding and generators.** Code generation is on Axa's "must not be added" list,
  and generated controllers become each service's own code. The deployment templates
  for Docker, nginx, and Lambda are product choices.
- **A generic `Cache` over several drivers, with no expiry.** Axa's Redis cache already
  requires expiry and falls back to the loader. A database cache driver adds load to
  the database the cache should protect.
- **A Redis job queue.** Axa keeps jobs in the database the work is about, so a job
  commits with its record. A second store loses that.
- **Shell commands from the scheduler.** Running shell text from configuration is a
  wide door for little gain. A job or a task does the same.
- **JWTs signed with a shared HMAC key.** Axa chose PASETO and Keycloak. Adding HS256
  JWTs would add a weaker way beside a stronger one, against P-10.
- **Multi-tenancy for SeaORM only.** Loco's `TenantEntity` works only with SeaORM, and
  Axa has four database crates of one shape. Tenant scoping that a developer must
  remember to call is weaker than row-level security in Postgres. If two services need
  tenants, the work is a design question first.
- **A Slack channel.** Slack is one product's API. A service that wants it can post
  through the outbox's publisher.
- **The powered-by header and the fallback HTML page.** Axa sends no product header,
  and answers an unmatched path with a problem.
