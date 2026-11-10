# Upgrading to Axa 0.12

This covers a service on 0.11. The database crates and features are now named for their
database and library, and there are crates for MySQL and SQLite beside Postgres. For a
service on Postgres, the upgrade is renaming; nothing it does changes.

## What will not compile

| Change | What to do |
|---|---|
| The feature `sqlx` is `postgres-sqlx`, `sea-orm` is `postgres-seaorm`, `diesel` is `postgres-diesel`, and `toasty` is `postgres-toasty` | Rename the feature in `Cargo.toml`: `axa = { version = "0.12", features = ["postgres-sqlx"] }` |
| `axa-sqlx` is `axa-postgres-sqlx`, `axa-seaorm` is `axa-postgres-seaorm`, `axa-diesel` is `axa-postgres-diesel`, and `axa-toasty` is `axa-postgres-toasty` | Rename a direct dependency in `Cargo.toml`, and `axa_sqlx::` to `axa_postgres_sqlx::` and the like in code |
| `axa::sqlx`, `axa::seaorm`, `axa::diesel`, and `axa::toasty` are `axa::postgres_sqlx`, `axa::postgres_seaorm`, `axa::postgres_diesel`, and `axa::postgres_toasty` | Rename the paths |
| `axa-transactional-outbox`'s feature `sea-orm` is `seaorm` | Rename it, if you turn it on directly. Through `axa`, `postgres-seaorm` turns it on |
| `axa::Error::Diesel` and `ModelError::Diesel` hold a `DieselError` | A `match` on them names `DieselError::Query` and `DieselError::Pool`. `?` still works as before |

## What changes with no compile error

**With more than one database feature on, the order is new.** Postgres wins, then MySQL,
then SQLite, and for each, sqlx, then SeaORM, Diesel, and Toasty. A service with only
Postgres features gets what it got before.

**A SeaORM feature builds sqlx too.** SeaORM built it already, so nothing new is
compiled, and `axa::Error` now has its `Sqlx` variant with SeaORM.

**A MySQL `DB_URL` must ask for TLS in production**, with `ssl-mode` of `REQUIRED`,
`VERIFY_CA`, or `VERIFY_IDENTITY`, unless it stays on this machine or `DB_ALLOW_INSECURE`
is set. A Postgres `DB_URL` is judged as before, by `sslmode`.

## New, and worth knowing

- MySQL: `mysql-sqlx`, `mysql-seaorm`, `mysql-diesel`, and `mysql-toasty`, with the same
  functions as their Postgres siblings. Each reads TLS from the URL's `ssl-mode`.
  `mysql-diesel` needs no `libmysqlclient`.
- SQLite: `sqlite-sqlx`, `sqlite-seaorm`, and `sqlite-diesel`, in the repository and not
  yet published, until sqlx releases the fix for RUSTSEC-2026-0459. A SQLite `DB_URL` is
  `sqlite://path`, and a missing file is made only with `?mode=rwc`.
- Each database crate says in its documentation what its database cannot do in the shared
  shape, such as MySQL's `truncate`, which cannot cascade.
