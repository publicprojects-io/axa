# Choosing a database and library

Axa has one crate for each pair of database and library, and an `axa` feature named for
the pair, such as `postgres-sqlx`. Turn on one. This page helps choose it. Every crate
has the same shape: a `Pool`, a `PoolConfig`, `connect`, `ping`, `migrate`, `pending`,
`truncate`, `dangerously_recreate`, `is_unique_violation`, and `quote_identifier`, so
the choice changes how a service writes queries, not how it boots, checks health, or
resets tests.

## The short answer

- **A new service, on Postgres: `postgres-sqlx`.** Every Axa feature works with it,
  and it is what the examples, the template, and `axa-test` use.
- **Leaving Loco, or wanting models: `postgres-seaorm`.** Loco's helpers keep their
  names, and the outbox can write in a SeaORM transaction.
- **No C client library allowed: `postgres-diesel` or `mysql-diesel`.** No `libpq` or
  `libmysqlclient` to link.
- **MySQL because the organization runs it: `mysql-sqlx`.**
- **One machine, no database server: SQLite**, once its crates are published (below).

## Pick the database

| | Postgres | MySQL | SQLite |
|---|---|---|---|
| Libraries | sqlx, SeaORM, Diesel, Toasty | sqlx, SeaORM, Diesel, Toasty | sqlx, SeaORM, Diesel |
| On crates.io | Yes | Yes | No: held for RUSTSEC-2026-0459 |
| Job queue, worker pool, schedules | `queue` feature, `axa-postgres-job-queue` | `axa-mysql-job-queue`, a crate on its own | `axa-sqlite-job-queue`, a crate on its own |
| Transactional outbox (`outbox`) | Yes | No | No |
| Idempotency keys (`idempotency`) | Yes | No | No |
| Shared rate limits | `ratelimit-postgres` | No, use `ratelimit-redis` | No |
| Row-level security for tenants | Yes, `require_row_security` | No | No |
| `axa-test` database per test | Yes | No | No |
| TLS | `sslmode` in the URL | `ssl-mode` in the URL, required in production | None: no network |

Postgres is the default for a reason: the outbox, idempotency keys, shared rate limits,
row-level security, and `axa-test`'s databases are all Postgres only. Choose MySQL when
the database is not yours to choose, and SQLite for a service on one machine that needs
none of those.

The SQLite crates build `libsqlite3-sys` 0.37, which has RUSTSEC-2026-0459: a crafted
database and `-wal` file pair can return uninitialized memory. sqlx caps it below the
fix, so `axa-sqlite-sqlx`, `axa-sqlite-seaorm`, and `axa-sqlite-diesel` are not on
crates.io until sqlx raises the cap. `axa-sqlite-job-queue` is published, with the
advisory stated in its README.

## Pick the library

| | sqlx | SeaORM | Diesel | Toasty |
|---|---|---|---|---|
| Queries | Your own SQL | Entities and a query builder | A typed query DSL, checked when it compiles | Models from a schema |
| `AppContext::db` | The sqlx pool | `DatabaseConnection` | A Diesel pool, over `diesel-async` | `toasty::Db` |
| Migrations | Up and down, with `revert` | Up and down, with `revert`, from the service's `Migrator` | Up and down, with `revert` | Forward only, no `revert` |
| Tenant transactions | `begin_with` sets `app.tenant` | `begin_with` | `set_local` in the service's transaction | `begin_with` |
| Outbox writes | `write` | `write_seaorm`, with the outbox's `seaorm` feature | No | No |
| C libraries | None | None | None, not even `libpq` or `libmysqlclient` | None |
| Maturity | Stable | 2.0 | 2.4 | Before 1.0, breaking releases expected |
| Other Axa features over sqlx | Shared | Shared: a SeaORM feature turns sqlx on too | Built alongside | Built alongside |

- **sqlx** suits a team that reads and writes SQL. Axa's own Postgres features, the job
  queue, outbox, idempotency keys, and shared limits, use sqlx, so a sqlx service builds
  one database library.
- **SeaORM** suits a team that wants models, or is moving from Loco: see
  [From Loco SeaORM into axa-postgres-seaorm](../from-loco-seaorm-into-axa-seaorm/index.md).
- **Diesel** suits a team that wants queries checked when they compile, or cannot link
  a C client library. Its connections run over `diesel-async`, so a handler awaits them
  as with any other library.
- **Toasty** is the newest, before 1.0, and its migrations cannot be undone. Choose it
  knowing each of its releases may break the service.

## A decision tree

```text
Is the database yours to choose?
├── No, it is MySQL ─────────────────────────────── mysql-sqlx (or a MySQL row below)
└── Yes
    ├── One machine, no server, and no outbox, idempotency keys, or tenants?
    │   └── SQLite, once published ──────────────── sqlite-sqlx
    └── Postgres
        ├── Moving from Loco, or want models? ────── postgres-seaorm
        ├── Want queries checked at compile time,
        │   or no C client library allowed? ──────── postgres-diesel
        ├── Want Toasty's models, and accept
        │   breaking releases and no revert? ─────── postgres-toasty
        └── Otherwise ───────────────────────────── postgres-sqlx
```

## Changing later

The crates share a shape, so moving from one library to another on the same database
changes queries and migrations, not the service around them. `AppContext::db` changes
type, which the compiler finds. Moving between databases also changes SQL dialects, and
loses whatever the new database lacks in the first table above.

## See also

- [Choosing features](../../docs/choosing-features/index.md), for every other feature
- [The README's database section](../../README.md), and each crate's own README
- [Section 4.21 of the spec](../../spec/index.md), for what each database cannot do
