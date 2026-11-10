# axa-sqlite-seaorm

SeaORM connections, migrations, schema helpers, and test resets on SQLite for Axum
services, from the [axa](https://github.com/publicprojects-io/axa) family.

It is the SQLite twin of `axa-postgres-seaorm`, and has the same shape as the other axa
database crates: a `Pool`, a `PoolConfig`, `connect`, `ping`, `migrate`, `revert`,
`pending`, `truncate`, `dangerously_recreate`, `is_unique_violation`, and
`quote_identifier`. The migration functions take the service's `Migrator` as a type.

What differs for SQLite:

- URLs are `sqlite://path` or `sqlite::memory:`. A missing file is made only when the
  URL ends `?mode=rwc`.
- Every connection turns on foreign keys and a busy timeout, and, for a file, WAL.
- `truncate` is `DELETE FROM` each table, in one transaction with foreign key checks
  deferred to its commit, and clears the tables' `sqlite_sequence` rows.
- `dangerously_recreate` drops every table but SQLite's own, on one connection with
  foreign keys off, then migrates.
- `schema::add_column` can add only what SQLite can: not a unique or primary key
  column, and, once the table has rows, not a required column without a default. No
  helper changes a column in place, which SQLite cannot do. A foreign key made by
  `schema::create_table` keeps no name.

It exists to make leaving [Loco](https://loco.rs) easy: a project keeps SeaORM
and its code mostly as it is, because the helpers Loco provided keep their names
and behaviour here.

| Loco | axa-sqlite-seaorm |
|---|---|
| `loco_rs::model::{ModelError, ModelResult}` | `axa::model`, the same for every database |
| `loco_rs::model::query::condition()` | `axa_sqlite_seaorm::query::condition()` |
| `loco_rs::schema` | `axa_sqlite_seaorm::schema` |
| `loco_rs::validation::Validatable` | `axa::validation::Validatable`, the same for every database |
| `loco_rs::db::truncate_table` | `axa_sqlite_seaorm::truncate_table` |

Enable it through the `sqlite-seaorm` feature of `axa`, which makes its `Pool` the
service's `AppContext::db`, or depend on this crate alone.

## Not yet published

The `libsqlite3-sys` 0.37 that sqlx 0.9 pins has the advisory RUSTSEC-2026-0459, so
this crate is held from crates.io until sqlx releases the fix.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
