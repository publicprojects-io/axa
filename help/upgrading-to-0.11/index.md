# Upgrading to axa 0.11

This covers a service on 0.10. Several changes compile cleanly and behave differently,
and the worst of them takes a service down at deploy, so read
[What changes with no compile error](#what-changes-with-no-compile-error) first, and do
the [deploy changes](#before-you-deploy) before the new binary goes out.

## Before you deploy

These are settings and probes outside the code. The new binary reads none of the old
names, so change them first.

| Old | New | If left alone |
|---|---|---|
| `DATABASE_URL` | `DB_URL` | Production refuses to boot. Development uses its default database |
| `AXA_TEST_DATABASE_URL` | `AXA_TEST_DB_URL` | The `#[ignore]` suites cannot find their database |
| `HTTP_CSP` | `HTTP_CONTENT_SECURITY_POLICY` | Production sends the strict default policy, which blanks a page that loads scripts or styles |
| `BODY_LIMIT_BYTES` | `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT` | The default limit, `Defaults::body_limit` |
| `HEADER_READ_TIMEOUT_SECONDS` | `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS` | 30 seconds |
| `LOCO_ENV` | `APP_ENV` | A debug build is development and a release build is production |
| Liveness probe on `/_ping` or `/_health` | `/_health/live` | `404`, so Kubernetes restarts the container again and again |
| Readiness probe on `/_readiness` | `/_health/ready` | `404`, so the pod never becomes ready |
| No startup probe | `/_health/startup`, optional | Nothing. Add one for a service that is slow to boot |

Update load balancers and uptime checks on those paths too. On Kubernetes, also set
`SHUTDOWN_DRAIN_SECONDS` to a little more than the cluster takes to remove a pod, and
keep `terminationGracePeriodSeconds` above it plus `SHUTDOWN_TIMEOUT_SECONDS`. Run the
new binary's `check` command against each environment's settings before you deploy it.

## What will not compile

| Change | What to do |
|---|---|
| `middleware::body_limit(bytes)` is now `body_limit(route, bytes)` | Change `post(upload).layer(body_limit(n))` to `body_limit(post(upload), n)` |
| `axa_seaorm::model` and `axa_seaorm::validation` are gone | Use `axa::model` and `axa::validation`, which every database feature now has. Code that already names `axa::model` does not change |
| `axa-problem-details-for-http-apis` is `axa-problem` again | Rename it in `Cargo.toml`, and `axa_problem_details_for_http_apis` to `axa_problem`, if you depend on it directly. `axa::problem` does not change |
| `config::Server` has `shutdown_drain_seconds`, and `config::Http` has `permissions_policy` | Set them, if you build one by hand |
| `axa_environment::VARIABLES` is `["APP_ENV"]` | Only code that indexes it changes |

## What changes with no compile error

**With both `sqlx` and `sea-orm` on, `AppContext::db` is a `PgPool`.** It used to be the
SeaORM connection. With more than one database feature on, `sqlx` wins, then `sea-orm`,
`diesel`, and `toasty`. A service that turns on both and uses `ctx.db` as a
`DatabaseConnection` turns off `sqlx`, or keeps its own SeaORM connection.

**A database that cannot serve answers `503`, not `500`.** A pool with no free
connection, a lost connection, or a read-only, shutting-down, or full server is a `503`
with the `code` `unavailable` and `Retry-After: 5`. Alerts that count `5xx` still see it.
Alerts that count only `500` do not.

**Readiness refuses from the moment shutdown begins.** `/_health/ready` answers `503`
with the `code` `shutting_down` while the service drains.

**New response headers.** Every response carries `cross-origin-opener-policy` and
`cross-origin-resource-policy` of `same-origin`, and a `permissions-policy` that turns off
the camera, microphone, location, payment, USB, sensors, and ad topics. A service whose
images, fonts, or files another site loads sets its own `cross-origin-resource-policy` on
those routes, such as `cross-origin`, as a handler's own value wins. A page that uses one
of those browser features sets `HTTP_PERMISSIONS_POLICY`, or blanks it.

**A response that sets a cookie is `no-store`,** unless its handler set its own
`cache-control`, and compression no longer touches any response that sets a cookie or is
`no-store`. A sign-in response is a little larger on the wire.

**A `401` from `axa-paseto-auth` or `axa-keycloak-auth` carries
`WWW-Authenticate: Bearer`.** A client that treated its absence as meaningful sees it now.

**The body limit binds a handler that reads `Body` itself.** Such a handler used to read
any size. A route that takes large uploads that way raises its limit with `body_limit`.

**Credential headers are marked sensitive** in the standard stack, so a layer of your own
that logged `authorization` or `cookie` no longer shows their values.

## New, and worth knowing

- `server::Shutdown`, an extractor that a WebSocket or a stream of server-sent events
  waits on, to end itself at shutdown.
- `RequestMetrics` in `axa-prometheus-metrics`: request durations by route template.
- The `diesel` and `toasty` features make `AppContext::db` their pool, and each database
  crate has the same shape: `Pool`, `connect`, `ping`, `migrate`, `pending`, `truncate`,
  `dangerously_recreate`, `is_unique_violation`, and `quote_identifier`.
- `axa-mysql-job-queue`, the job queue on MySQL, as a crate on its own.
- The health routes, and why each probe asks what it asks, are in
  [Health routes](../../spec/health/index.md).
