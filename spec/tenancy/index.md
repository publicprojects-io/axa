# Multi-tenancy

A study of what Axa could add for a service that many tenants share, written on
13 November 2026. TN-1 is built. The rest is not yet. It proposes capabilities, each with an id
(TN-1 and on) that [`tasks.md`](../../tasks.md) uses. The first consumer is
[`examples/multi-tenant`](../../examples/multi-tenant/README.md), which does by hand what
the first few would give.

## Why it has a safety case

The worst failure a shared service has is one tenant reading or changing another's data.
It is the first risk in the OWASP API Security Top 10, as broken object-level
authorization, and it is the kind of failure a hazard log or a security review names. So
the parts that keep tenants apart pass P-2 on one consumer with that safety case. Parts
that only make tenancy more convenient wait for two.

## Where Axa stands

From Axa's own code, at the commit this page was written on:

| Part | What it does for tenants today |
|---|---|
| `axa-paseto-auth` | A verified token's `attrs` can carry a tenant, as `"tenant": ["acme"]`, and the ABAC policy can match on it. Nothing reads it as a tenant, so each service writes its own extractor. |
| `axa-keycloak-auth` | The same `attrs` map, from Keycloak's claims. |
| `axa-request-rate-limit` | `by_key` counts by any key a service returns, so a limit per tenant is possible. There is no example. |
| `axa-idempotency-key` | Keys are scoped by the text a service returns for a request. Its documentation already says a service with tenants should return a text naming both. |
| `axa-cursor-pagination` | A cursor is signed over its position, the path, and the filters. The tenant is not among them unless a service makes it a filter. |
| `axa-redis-cache` | One prefix per cache, usually the service name. Nothing stops a key from one tenant being read for another. |
| Job queues and outbox | No tenant on a job or an event. A job that should run in a tenant has to carry it in its arguments. |
| Logging | The request span holds the method, route, and request id. No caller or tenant. |
| Database crates | A connection runs as whatever role `DB_URL` names. Nothing checks whether row-level security can apply to it. |

## What others do

- **Rails, `acts_as_tenant`**: a current tenant set per request, a default scope on each
  tenant model, and an ActiveJob extension that saves the tenant into a job and restores
  it when the job runs. Scoping every query by default is convenient, and it is the kind
  of database abstraction Axa does not add. Carrying the tenant into jobs is the part
  worth taking.
- **Loco, `TenantEntity`**: SeaORM-only scoping that a developer calls, studied in
  [the Loco page](../compare/loco/index.md). It was left out there because it covers one
  of Axa's four database libraries, and a scope a developer must remember is weaker than
  one the database enforces.
- **Laravel, Tenancy for Laravel**: tenants found from the domain or subdomain, and mostly
  a database per tenant, switched per request. A domain can say which tenant a request
  means, but it says nothing about whether the caller belongs to it.
- **Windmill**: Postgres row-level security, with the tenant set per transaction, studied
  in [the Windmill page](../compare/windmill/index.md) as `begin_with`.
- **Postgres row-level security in practice**: the reports agree on four traps. A plain
  `SET` lasts for the session, so through a pool it leaks into the next request, and
  `set_config(name, value, true)` or `SET LOCAL` lasts only for the transaction. A table's
  owner skips its policies unless the table has `FORCE ROW LEVEL SECURITY`. A superuser or
  a role with `BYPASSRLS` skips them always. A pooler in statement mode can run one
  transaction's statements on different connections.
- **AWS, SaaS tenant isolation and the SaaS lens**: isolation is judged resource by
  resource, the tenant comes from the caller's identity, and each tenant gets its own
  throttle, so one tenant cannot use up a shared limit, the "noisy neighbour".

## What Axa could add

| Id | Capability | Why | Effort | Case |
|---|---|---|---|---|
| TN-1 | A `Tenant` extractor in both auth crates, from the verified token only | Every service writes this, and the mistakes are taking it from a header or accepting a token with two tenants | S | Safety |
| TN-2 | A tenant transaction in the Postgres crates: `begin_in(pool, &[("app.tenant", id)])` | Row-level security needs the setting per transaction, bound, on the same connection as the queries | S | Safety |
| TN-3 | A boot check that row-level security can apply: production refuses a role that is a superuser or has `BYPASSRLS`, and names any listed table without `FORCE ROW LEVEL SECURITY` | The policy silently does nothing otherwise, and tests run as a superuser pass anyway | S | Safety |
| TN-4 | Isolation helpers in `axa-test`: tokens minted with given `attrs`, and a check that a route answers `404` for another tenant's id | A cross-tenant test is the one that proves the fences, and each service writes it from nothing | M | Safety |
| TN-5 | A tenant on a job: an optional `tenant` column on the job queues, set at enqueue and handed to the worker | A job runs outside any request, so the tenant is lost unless the queue carries it | M | Two consumers |
| TN-6 | A cache scoped to a tenant: `cache.for_tenant(&tenant)`, putting the tenant in every key | One tenant's cached answer served to another is a leak, and a prefix written by hand is easy to forget | S | Safety |
| TN-7 | A limit per tenant, documented and tested, with `by_key` on `(tenant, class)` beside the limit per client | The noisy neighbour, without a new crate | S | Two consumers |
| TN-8 | A cursor bound to a context, such as the tenant, inside its signature | A cursor from one tenant is then refused in another, even where the queries forget | S | Two consumers |
| TN-9 | A tenant field on the request span, opt-in, never a metrics label | Logs filtered by tenant help operators. A tenant label on metrics grows without bound | S | Two consumers |
| TN-10 | A tenant on an outbox event, for routing and ordering per tenant | Consumers downstream often split by tenant | M | Two consumers |
| TN-11 | A suspended tenant refused with `403`, code `tenant_suspended`, from a check the service passes in | Suspension belongs to the service that issues tokens, but tokens live for minutes | S | Two consumers |

**TN-1, the tenant from the token.** Done, unreleased, as T-43: PST-13 to PST-15 and
KCK-14 and KCK-15. `Tenant` reads the `Allowed` the guard left, so it
exists only after the policy allowed the request. It takes exactly one value of a named
attribute, `tenant` unless the guard's config names another, and refuses a value that
is not a tenant id: lower-case letters, digits, and hyphens, 1 to 63 characters, not
starting or ending with a hyphen, as a DNS label is. No token is `401`, and a token with no tenant, two tenants, or a bad one is
`403`, code `no_tenant`. There is no way to take a tenant from a header, a path, or a
host, as P-10 asks: a service that routes by subdomain compares the host's tenant with
the token's and refuses a mismatch.

**TN-2, the tenant transaction.** In `axa-postgres-sqlx`, a function that opens a
transaction and runs `SELECT set_config($1, $2, true)` for each pair, so each value is
bound and lasts until the transaction ends. The policies stay in the service's own
migrations, so this adds no query layer. SeaORM and Diesel follow with the same shape.
MySQL has no row-level security, so the MySQL crates have no TN-2, and a MySQL service
keeps the first fence only.

**TN-3, the boot check.** `SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname =
current_user`, and `relforcerowsecurity` from `pg_class` for each table the service names.
Production refuses to boot when either fails. Development warns, so that a local
superuser still works, but `axa-test` can run the isolation tests as a role the policies
apply to.

**TN-4, isolation tests.** For `axa-paseto-auth`, a test key and a `token(attrs)` that the
guard accepts, as `examples/multi-tenant` writes by hand. Then a helper that, given a
route that creates a resource and the routes that read, change, and delete it, runs them
as a second tenant and expects `404` from each.

**TN-5, jobs.** `perform_later_in(&tenant, …)`, a nullable `tenant` column, and the tenant
handed to the worker beside the arguments, so that the worker opens its own tenant
transaction. Claiming stays fair across jobs, not tenants. Fair claiming per tenant is a
further step, once a service shows one tenant starving the rest.

## What not to add

- **A tenant from a header, path, subdomain, or body alone.** Any caller can name any
  tenant. This is what `examples/multi-tenant` did before it was rewritten.
- **Scoping every query by default.** It is a database abstraction, which Axa does not
  add, and it hides the filter that a reviewer needs to see.
- **A schema or database per tenant, made at run time.** Creating schemas from request
  data is a door for injection and exhaustion, migrations must then run once per tenant,
  and pools multiply. A tenant that needs its own database is a deployment of its own.
- **A tenant label on metrics.** Every new tenant is a new series.
- **Tenant administration**, such as creating and suspending tenants. It belongs to the
  service that issues tokens.

## Open questions

- Keycloak can put a user's organizations in a token. Whether TN-1 should read that claim
  as well as `attrs`, and in what shape, needs checking against Keycloak's documentation.
- Whether TN-3 should also refuse a connection through a pooler in statement mode, which
  Postgres cannot see from its side.
- Whether TN-5's tenant belongs in the job queue's own table or in the arguments a service
  already controls, given that a queue with a tenant column invites a fair-claiming
  feature next.

## Sources

- [OWASP API Security Top 10, API1: broken object-level authorization](https://api-security.owasp.org/editions/2023/en/0xa1-broken-object-level-authorization)
- [acts_as_tenant](https://rdoc.info/gems/acts_as_tenant)
- [Tenancy for Laravel](https://tenancyforlaravel.com/)
- [AWS, SaaS tenant isolation strategies](https://docs.aws.amazon.com/whitepapers/latest/saas-tenant-isolation-strategies/saas-tenant-isolation-strategies.html)
- [AWS Well-Architected SaaS lens, PERF 1](https://wa.aws.amazon.com/saas.question.PERF_1.en.html)
- [Common Postgres row-level security footguns](https://www.bytebase.com/blog/postgres-row-level-security-footguns.md)
- [Postgres RLS multi-tenancy: two traps that silently disable your policies](https://dev.to/wenceslaudev/postgres-rls-multi-tenancy-two-traps-that-silently-disable-your-policies-5gn8)
- [PostgreSQL documentation, row security policies](https://www.postgresql.org/docs/current/ddl-rowsecurity.html)
