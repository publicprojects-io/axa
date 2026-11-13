# Multi-tenant

A todo API that many tenants share, on Postgres, with Axa's `postgres-sqlx` and `paseto`
features. One table holds every tenant's rows, and no tenant can see or change another's.

## How a tenant is chosen

**From the caller's verified token, and nowhere else.** The auth service puts the
tenant in the token's `attrs`, as `"tenant": ["acme"]`. The `Tenant` extractor reads it
from what the PASETO guard verified and allowed, and a handler that takes `Tenant` cannot
run without one.

| The token | The answer |
|---|---|
| None, or not valid | `401` |
| Valid, with no `tenant`, more than one, or one that is not a tenant id | `403`, code `no_tenant` |
| Valid, with one tenant, for an action the policy refuses | `403` |
| Valid, with one tenant, allowed | The request runs in that tenant |

A tenant id is lower-case letters, digits, and hyphens, at most 63 characters, not
starting with a hyphen. The table checks this too.

Never take the tenant from a header, a path, a subdomain, or a body that the caller
chooses: any caller could then name any tenant. A subdomain can say which tenant a
request means, but the token still has to say the caller belongs to it.

## Two fences

1. **Every query filters on the tenant.** Each row has a `tenant_id`, the primary key is
   `(tenant_id, id)`, and every `SELECT`, `UPDATE`, and `DELETE` says
   `WHERE tenant_id = $1`. Another tenant's todo is `404`, the same as a missing one, so
   ids leak nothing.
2. **Postgres row-level security** refuses another tenant's rows even to a query that
   forgets the filter. Each request runs in a transaction that sets `app.tenant` with
   `set_config('app.tenant', $1, true)`: bound, not formatted into the SQL, and ended
   with the transaction, so a pooled connection never carries a tenant into the next
   request. The policy is in [the migration](migrations/20261113120000_create_todos.sql).

Row-level security does not apply to a superuser or a role with `BYPASSRLS`, so connect
as a role that is neither:

```sql
CREATE ROLE todo_app LOGIN PASSWORD 'todo_app';
GRANT CREATE ON SCHEMA public TO todo_app;   -- to run the migration as this role
```

## Run it

```sh
podman-compose up -d postgres                       # from the repository root
podman-compose exec postgres psql -U axa -d axa_development \
  -c "CREATE ROLE todo_app LOGIN PASSWORD 'todo_app'" \
  -c "GRANT CREATE ON SCHEMA public TO todo_app"
cd examples/multi-tenant
export DB_URL=postgres://todo_app:todo_app@localhost:5432/axa_development
export PASETO_KEYS_FILE=./keys.json PASETO_ISSUER=authentication-service PASETO_AUDIENCE=multi-tenant
cargo run
```

`keys.json` is the auth service's public key set, a JSON document with a `keys` array of
Ed25519 keys. Every request to `/api/todos` needs a token from that service:

```sh
curl localhost:1241/api/todos -H "authorization: Bearer $TOKEN"
```

## Routes

| Method | Path | Needs | Answer |
|---|---|---|---|
| `GET` | `/api/todos` | Any verified caller | Up to 100 of the tenant's todos |
| `POST` | `/api/todos` | `access=write` | The new todo |
| `GET` | `/api/todos/{id}` | Any verified caller | The todo, or `404` |
| `PATCH` | `/api/todos/{id}` | `access=write` | The changed todo, or `404` |
| `DELETE` | `/api/todos/{id}` | `access=admin` | `200`, or `404` |

The "Needs" column is the default ABAC policy, which the guard applies before a handler
runs.

## What it does not do

- **Per-tenant limits.** Axa's `ratelimit` and `idempotency` features key on what a
  service returns for a request, so a service can key them on the tenant as well as the
  caller. This example does not.
- **Tenant administration.** Creating, suspending, and deleting tenants belongs to the
  service that issues tokens.
- **A schema or database per tenant.** One table with row-level security is simpler to
  migrate and to pool. A tenant that needs its own database is a deployment of its own.

## Test it

```sh
cargo test
```

The tests mint tokens with a throwaway key, and check that a caller with no tenant is
refused before the database is asked. The test that one tenant never sees another's
rows needs Postgres, as a role row-level security applies to:

```sh
AXA_TEST_DB_URL=postgres://todo_app:todo_app@localhost:5432/multi_tenant_test cargo test -- --ignored
```

It drops and recreates `todos` and `_sqlx_migrations`, so give it a database of its own.
