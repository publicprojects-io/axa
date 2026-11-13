# Plan: what Axa adds next

This plan comes from studying twelve open-source Rust web projects from their source, eight
services on Axum and four other frameworks. Each study is in [`spec/compare/`](spec/compare/index.md),
and the work is in [`tasks.md`](tasks.md). It was written on 10 November 2026.

## What the studies found

`axa` is already ahead of every project studied in several places: RFC 9457 problems that
send no internal text, client addresses trusted only from named proxies, three health
probes, timeouts per route, idempotency keys that fingerprint the request, an outbox that
keeps a record, audited cancel and retry of jobs, signed cursors, OpenAPI drift checks, and
production refusing unsafe settings. Most of the projects get at least one of these wrong:
Kanidm and Rocket trust a forged forwarding header, Relay and Poem send error text to clients,
Poem's CSRF key defaults to zeros, Svix opens CORS to every origin, and Atuin's login says
which usernames exist. Each page has a "should not copy" section, and those are as useful as
the gaps.

The gaps fall into a few groups. The most important are not new features but holes in what
`axa` already claims to do:

- **A `401` without `WWW-Authenticate`.** Neither auth crate sends one, which RFC 9110
  requires.
- **Readiness during shutdown.** `/_health/ready` stays `200` after `SIGTERM`, so a draining
  pod keeps getting traffic.
- **A body limit with a gap.** `DefaultBodyLimit` binds the extractors only, so a handler
  that reads `Body` itself is not limited, and a compressed body is not limited at all
  because `axa` does not decompress requests.
- **Database outages answer `500`.** A pool with no free connection, a closed connection, or
  a read-only database is the service's own trouble, and `503` with `Retry-After` is the
  honest answer.

Beyond those, the same few capabilities came up again and again, in projects that share
nothing else:

| Capability | Projects that point to it |
|---|---|
| An outbound HTTP client that refuses private and metadata addresses | Svix, Windmill, tuwunel |
| Signing and verifying webhooks | Svix, Windmill, Loco |
| Request metrics by route template | Atuin, Poem, Sentry Relay |
| A database per test in `axa-test` | crates.io, Loco, Windmill |
| Bounded multipart uploads, spooled to disk | actix-web, Poem, Rocket |
| A guard against cross-site writes, from `Sec-Fetch-Site` and `Origin` | Topcoat, Windmill, actix-web |
| A shutdown signal a long-lived stream can wait on | actix-web, Rocket, tuwunel |
| More security headers: `Permissions-Policy`, COOP, CORP, and `no-store` | Kanidm, Rocket |
| A mail crate with SMTP, file, and memory transports | Loco, Topcoat, crates.io |
| Rate limits shared across replicas | crates.io, actix-web |

## How the plan is ordered

Every item is held to the principles in [`spec/index.md`](spec/index.md), and three of them
decide the order:

- **P-1, library not framework.** Nothing here adds a hook, a macro, or a configuration file.
  Each item is a function, a type, or a layer a service calls or adds itself.
- **P-2, two consumers first, or one with a safety case.** An item that fixes a hole in what
  `axa` already promises has its case already. A new capability waits until two services
  ask, or one has a safety case, and the tasks say which is which.
- **P-6, new dependencies are raised first.** Each item that needs one names it, and waits
  for a yes.

So the plan has three stages.

### Stage 1: close the holes, with no new dependency

**Done in Axa 0.11.0**, with the spec ids in [`tasks.md`](tasks.md).

These make what `axa` already does correct, so they need no new consumer. Most are small,
and several are breaking, so they belong in 0.11 with the other breaking changes on main:

1. `WWW-Authenticate` on every `401` from both auth crates.
2. Readiness answers `503` once shutdown begins, and a shutdown signal that handlers, and
   streams most of all, can wait on.
3. A body limit that binds a streamed `Body` too, with `RequestBodyLimitLayer`.
4. `503` with `Retry-After` for a pool timeout, a closed connection, and a read-only
   database, across all four database crates.
5. `Permissions-Policy`, `cross-origin-opener-policy`, and `cross-origin-resource-policy`
   in the standard security headers, each overridable as the existing ones are.
6. `no-store` on responses that set a cookie or carry a credential, and no compression of
   them, against BREACH.
7. Credential headers marked sensitive in the standard stack, so no layer logs them.
8. Request metrics by route template in `axa-prometheus-metrics`, with one `unmatched` label
   for requests that match no route. It needs nothing new, and without it the crate measures
   nothing.

### Stage 2: capabilities with a clear safety case

These are new, but each closes a hazard that a service otherwise meets alone and usually gets
wrong. Several need a new dependency, which is raised first:

1. **An outbound HTTP client that refuses private addresses** after DNS lookup and on every
   redirect, and connects only to the addresses it checked. Three projects built one, and
   two rank server-side request forgery among their top threats. New crate, `axa-http-client`,
   over `reqwest`.
2. **Webhook signing and verifying** to the Standard Webhooks scheme, with a timestamp window
   and constant-time comparison. New crate, `axa-webhooks`, over `hmac` and `sha2`.
3. **A cross-site write guard** from `Sec-Fetch-Site`, falling back to `Origin`, that lets
   non-browser clients through and has a per-route opt-out for signed webhooks. No new
   dependency.
4. **Bounded multipart uploads**: limits on total size, memory, parts, and each field,
   spooled to a temporary file, with a cleaned file name and a safe `Content-Disposition`
   for downloads.
5. **A database per test in `axa-test`**: a schema per test from a template migrated once,
   so database tests run in parallel.
6. **Per-connection settings** in the database crates: `statement_timeout`,
   `application_name`, and an optional read-only mode, plus recovery from the stale
   prepared statements a migration leaves behind.
7. **Readiness that waits for migrations**: with migrations still pending, a new pod is not
   ready and its workers wait.
8. **Request body decompression** with the limit counted on decoded bytes.
9. **A public settings reader**, so a service's own variables get the same production
   refusals and redaction as `axa`'s, and secrets read from `_FILE` paths.
10. **Field paths in JSON body errors**, as a JSON Pointer that quotes nothing sent.

### Stage 3: wait for two consumers

Each of these is useful, and each is the kind of thing P-2 exists to hold back until services
ask: a mail crate, object storage, rate limits shared through Postgres or Redis, waking the
Postgres queue with `LISTEN` and `NOTIFY`, deduplicated enqueue, cron schedules, retry jitter
and `Retry-After` in the queues, an internal listener for health and metrics, a static
credential guard for operator routes, opaque API tokens, a session deadline from sign-in, a
tri-state `Patch<T>`, a form context for Tera pages, a `health` command for images with no
`curl`, Tokio runtime metrics, PROXY protocol, an OpenAPI conformance check in tests, and
captured logs and mail in `axa-test`.

## Multi-tenancy

A [later study](spec/tenancy/index.md), from 13 November 2026, looked at what a service
that many tenants share needs from Axa, against Rails' `acts_as_tenant`, Loco, Tenancy for
Laravel, Windmill, Postgres row-level security as it is used in practice, and AWS's SaaS
isolation guidance. Axa already has most of the pieces: a token's `attrs` can carry the
tenant, a rate limit and an idempotency key can be keyed on it, and Postgres can enforce
it. What is missing is the glue, which every service now writes by hand, and the checks
that the glue cannot quietly fail.

So the tenancy work comes in two parts:

1. **Keep tenants apart**, with a safety case under P-2: a `Tenant` extractor that reads
   only the verified token (T-43), a transaction that sets the tenant for row-level
   security (T-44), a boot check that the policies can apply to the role the service runs
   as (T-45), isolation helpers in `axa-test` (T-46), and a cache scoped to a tenant
   (T-47). [`examples/multi-tenant`](examples/multi-tenant/README.md) does each of the
   first three by hand today, and is the first consumer.
2. **Wait for two consumers**: a tenant on a job and on an outbox event, a limit per
   tenant, a cursor bound to its tenant, a tenant on the request span, and refusing a
   suspended tenant (T-48 to T-53).

It leaves out a tenant from a header or a host alone, scoping every query by default, a
schema or database made per tenant at run time, and a tenant label on metrics. The study
gives the reasons.

## What the plan leaves out

The studies agree on what not to take. `axa` keeps no route macros, attribute macros, or
routes derived from modules (Rocket, Poem, Topcoat, crates.io). It keeps no lifecycle hooks,
initializers, fairings, or configuration files with profiles (Loco, Rocket). It runs its own
queues on the database, not on Redis or RabbitMQ (Svix, Loco). It does not own the server or
TLS (actix-web, Rocket), and it does not trust forwarding headers from anyone (Kanidm, Rocket,
Poem, actix-web). Each page gives its reasons.

## Risks

- Stage 1 changes defaults and status codes, so it is breaking, and services will see `503`
  where they saw `500`, and new headers. The changelog says so, as it does for the other
  0.11 changes.
- The outbound client and the webhook crate are security code. Each needs its own threat
  model in the spec, tests against real resolvers and redirects, and review before release.
- Some findings rest on one reading of another project's code. Where a page could not confirm
  a point, it says so, and a task built on such a point checks it first.
