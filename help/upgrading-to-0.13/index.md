# Upgrading to Axa 0.13

This covers a service on 0.12. Most services upgrade by changing the version. A service
that builds a configuration struct field by field adds the new fields, and a service
that takes cross-site writes, such as an identity provider's `form_post` callback, lists
those routes.

```toml
[dependencies]
axa = "0.13"

[dev-dependencies]
axa-test = "0.4"
```

## What will not compile

| Change | What to do |
|---|---|
| `config::Http` has `cross_site_paths` | Add `cross_site_paths: Vec::new()` where you build an `Http` by hand |
| `config::Database` has `statement_timeout_ms`, `application_name`, and `read_only` | Add `statement_timeout_ms: 0`, `application_name: String::new()`, and `read_only: false` where you build one by hand, or build it with `Config::from_env` |
| Each Postgres crate's `PoolConfig` has the same three fields | Build it with `PoolConfig::new` and set what you need, or add the three fields |
| `GuardConfig`, in `axa-paseto-auth` and `axa-keycloak-auth`, has `tenant_attribute` | Build it with `GuardConfig::new`, or add `tenant_attribute: "tenant".into()` |
| `axa_redis_cache::Error` has `TenantScope` | Add a wildcard arm to a `match` on it |
| `axa-test` 0.4 is the one for `axa` 0.13 | Set `axa-test = "0.4"` in `[dev-dependencies]` |
| A direct dependency on a database crate moves with `axa` | `axa-postgres-sqlx = "0.2"`, `axa-postgres-seaorm = "0.4"`, `axa-postgres-diesel = "0.3"`, `axa-postgres-toasty = "0.2"`, `axa-paseto-auth = "0.7"`, `axa-keycloak-auth = "0.5"`, `axa-redis-cache = "0.2"` |

## What changes with no compile error

**The standard stack refuses a cross-site write.** An unsafe method, or a WebSocket
upgrade, that a browser marks `sec-fetch-site: cross-site` is `403`, code
`cross_site_write_refused`. With no `sec-fetch-site`, one whose `origin` is not the
request's own host is `403`, code `origin_mismatch`. A request with neither header, as
from a server or `curl`, passes. Two kinds of request need a setting:

- **A browser page on another origin that writes to this service** passes once that
  origin is in `CORS_ALLOWED_ORIGINS`, as it already is if CORS lets the page read the
  answer.
- **A route that takes a cross-site form post by design**, such as an OpenID Connect
  `form_post` callback or a payment provider's return, is listed in
  `config.http.cross_site_paths`, set in code, matched exactly.

**A compressed request body is decoded.** `gzip`, `deflate`, `br`, and `zstd` bodies
reach the handler decoded and without `content-encoding`, and the body limit counts the
decoded bytes, so a small body that expands past it is `413`. An encoding it cannot
decode is `415`. A handler that decoded a body itself now gets it decoded already.

**`DB_URL` may come from a file, and setting both is refused.** `DB_URL_FILE` names a
file holding the connection string, as a mounted secret is. With both set, or a file
that cannot be read, the service does not boot, in any environment.

**A rejected JSON body says where.** A body that parses but has a value of the wrong
type gets `errors: [{"pointer": "#/items/1/count"}]` in its `422` problem. A client that
compared whole problem bodies sees the new member.

**Postgres connections are named.** Every connection gets `application_name`, the
database name stem by default, so `pg_stat_activity` names the service. Set
`DB_APPLICATION_NAME` to choose another.

## New, and worth knowing

- **`config::Settings`** reads a service's own variables with Axa's rules: problems
  named all at once, production refusing to boot on any, and `secret` reading `NAME` or
  `NAME_FILE`.
- **Tenancy**: a `Tenant` extractor in both auth crates, `begin_with` for Postgres
  row-level security, `require_row_security` at boot, `Cache::for_tenant`, and
  `axa_test::isolation::assert_hidden`. [`examples/multi-tenant`](../../examples/multi-tenant)
  puts them together.
- **Postgres session settings**: `DB_STATEMENT_TIMEOUT_MS`, `DB_APPLICATION_NAME`, and
  `DB_READ_ONLY`.
- **Readiness waits for migrations**, with `Checks::migrations` and
  `server::run_once_migrated`, and a stale prepared statement after a migration is a
  `503` to retry rather than an error on every pooled connection.
- **New features**:
  - `upload`, bounded multipart uploads;
  - `webhooks`, Standard Webhooks signing and verifying, in the new `axa-webhooks`;
  - `http-client`, an outbound client that refuses private addresses, in the new
    `axa-http-client`.
- **`format::download`** sends a file with a safe `Content-Disposition`.
- **`axa-test`** has `postgres::TestDatabase`, a migrated database for each test, and
  `paseto::TestKeys`, tokens signed under a throwaway key.
