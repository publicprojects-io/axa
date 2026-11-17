# axa Roadmap

This document outlines the planned development path for axa. See
[`plan.md`](plan.md) and [`tasks.md`](tasks.md) for detailed task breakdowns.

---

## Current Version: 0.17.0

---

## Recently Completed

### Version 0.17.0
- A review for security and stability, with the fixes in the CHANGELOG: an empty public
  prefix no longer opens every path, Keycloak refusals say only `invalid token`, shared
  rate limits purge by their own windows and hash keys alike on every build, job
  heartbeats are bounded and stop before another worker may take a run, and shutdown
  stops hung jobs
- Rust 1.98, Diesel 2.4 with `diesel-async` 0.10, and `axum_session` 0.23

### Version 0.16.1
- **`axa-minijinja-templates`**: MiniJinja (Jinja2) templates, behind the `minijinja`
  feature

### Version 0.16.0
- The rest of Stage 3, which the maintainer asked to go ahead on 2026-11-15 and
  2026-11-16:
  - **T-35**: Tri-state `Patch<T>`
  - **T-34**: Session deadline from sign-in
  - **T-32**: Static token or Basic credential guard for operator routes
  - **T-37**: `health` command for images without `curl`
  - **T-42**: Trailing-slash policy with `308` redirects
  - **T-28**: Retry jitter and job-chosen delays
  - **T-41**: Captured logs and mail in `axa-test`
  - **T-26**: Wake the Postgres queue with `LISTEN` and `NOTIFY`
  - **T-33**: Opaque API tokens with scopes and expiry, `axa-api-tokens`
  - **T-31**: Internal listener for health and metrics, `INTERNAL_PORT`
  - **T-36**: `extract::Form` and a form context for Tera pages
  - **T-30**: Background migrations for SQL that cannot run in a transaction
  - **T-25**: Rate limits shared across replicas in Postgres or Redis
  - **T-38**: Tokio runtime and task metrics, `prometheus-tokio`
  - **T-39**: PROXY protocol from `PROXY_PROTOCOL_FROM`
  - **T-40**: OpenAPI conformance checks in `axa-test`

### Version 0.15.0
- Four Stage 3 tasks that `crm`, `ppm`, and `wpm` asked for:
  - A job queued once per key while one is queued or running, `NewJob::unique` (T-27)
  - Cron schedules, `Schedule::cron`, over `croner` (T-29)
  - `axa-mail`, over `lettre` (T-23)
  - `axa-storage-with-apache-arrow-and-aws`, over `object_store` (T-24)

### Version 0.14.0
- `axa::webhooks::Verified` with `AppContext` as the state, taking the `Verifier` from
  `ctx.shared_store` and failing closed without one (WHK-6, WHK-7), with
  `axa-webhooks` 0.1.1
- The rest of multi-tenancy, which three services asked for (T-48 to T-53):
  - A limit per tenant (T-49)
  - A tenant on the request span, never a metrics label (T-51)
  - A suspended tenant refused with `403`, code `tenant_suspended` (T-53)
  - A cursor bound to the tenant (T-50)
  - A tenant on a job (T-48)
  - A tenant on an outbox event (T-52)
- MySQL session settings: a statement timeout and read-only mode (T-17)
- Stale plans retired in the SeaORM, Diesel, and Toasty Postgres pools (T-18)

### Version 0.13.0
- Stage 2 tasks (capabilities with a safety case):
  - Cross-site write guard in the standard stack (T-13)
  - Bounded multipart uploads, `axa::upload` (T-14)
  - Safe downloads, `format::download` (T-15)
  - A Postgres database per test in `axa-test` (T-16)
  - Postgres session settings (T-17), and MySQL's in 0.14.0
  - Recovery from stale prepared statements (T-18)
  - Readiness that waits for migrations (T-19)
  - Decoded compressed request bodies (T-20)
  - `config::Settings`, with `_FILE` secrets (T-21)
  - JSON Pointers in body errors (T-22)
  - `axa-http-client`, refusing private addresses (T-11)
  - `axa-webhooks`, Standard Webhooks signing and verifying (T-12)
- Multi-tenancy (T-43 to T-47): a `Tenant` extractor, row-level security
  transactions and boot checks, a tenant-scoped cache, and isolation test helpers

### Version 0.12.1
- MySQL and SQLite support for all database features
- Renamed database crates to database-specific names (e.g., `axa-postgres-sqlx`)
- Documentation restructuring into modular docs
- Visual architecture diagrams
- Podman-based development infrastructure

### Version 0.11.0
- Stage 1 tasks (closing holes):
  - `WWW-Authenticate` on every `401` from auth crates (T-1)
  - Readiness answers `503` during shutdown (T-2)
  - Body limit binds streamed bodies (T-4)
  - `503` with `Retry-After` for database outages (T-5)
  - Additional security headers: Permissions-Policy, COOP, CORP (T-6)
  - `no-store` on responses that set cookies (T-7)
  - Sensitive credential headers marked (T-9)
  - Request metrics by route template (T-10)

---

## Next

Stages 1 and 2 are done, and all of Stage 3 but T-54, held because its dependencies were
declined on 2026-11-16.

`reqwest` 0.13 for `axa-http-client` waits for a choice of its own trust roots and TLS
provider.

---

## Future: Version 1.0.0

### Stage 3: Waiting for Two Consumers

Each item waits until two services request it, or one has a compelling safety case:

- **T-54**: SQL Server, over `tiberius`, held: its dependencies were declined on 2026-11-16

---

## Principles Guiding Development

1. **Library, not framework** (P-1): axa remains a library. No hooks, macros, or
   code generation.
2. **Two consumers first** (P-2): Code is added only once two services need it,
   or one has a compelling safety case.
3. **Fail closed** (P-3): Production refuses unsafe configurations.
4. **Generic errors** (P-4): Internal failure text is never sent to clients.
5. **New dependencies raised first** (P-6): New dependencies are discussed before
   implementation.

---

## How to Influence the Roadmap

1. **Open issues** describing your use case
2. **Submit PRs** with implementations and tests
3. **Join discussions** (see [CONTRIBUTING.md](CONTRIBUTING.md))
4. **Provide feedback** on open RFCs

---

## Versioning Policy

- **Patch versions** (0.12.x): Bug fixes, non-breaking improvements
- **Minor versions** (0.x.0): Breaking changes, new features
- **Major version 1.0**: Stabilization of API, comprehensive feature set

---

## See Also

- [plan.md](plan.md) - Strategic planning document
- [tasks.md](tasks.md) - Detailed task list with effort estimates
- [CHANGELOG.md](CHANGELOG.md) - Release history
- [spec/index.md](spec/index.md) - Technical specification
