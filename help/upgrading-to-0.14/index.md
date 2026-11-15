# Upgrading to Axa 0.14

This covers a service on 0.13, and is written as 0.14 is built. Most services upgrade by
changing the version. A service that builds a configuration struct field by field adds
the new fields.

## What will not compile

| Change | What to do |
|---|---|
| `config::Http` has `log_tenant` | Add `log_tenant: false` where you build an `Http` by hand |
| The outbox's `Event` and `Claimed` have `tenant`, and `Refused` has `NotATenant` | Build an `Event` with `Event::new`, or add `tenant: None`; add an arm for `NotATenant` to a `match` on `Refused` |
| `JobInfo` has `tenant`, and `SETUP_STATEMENTS` in each job queue has one more statement | Nothing, unless you match on the array's length |
| `PoolConfig` in each MySQL crate has `statement_timeout_ms` and `read_only` | Build one with `PoolConfig::new` and `..`, or add `statement_timeout_ms: 0, read_only: false` |

## What changes with no compile error

**The outbox has a `tenant` column**, on `axa_outbox_events` and `axa_outbox_owed`,
and a second form of `axa_outbox_fingerprint` that covers it. `Outbox::connect` adds
them. A service on `Outbox::connect_existing` runs `SETUP_STATEMENTS` again first, as
`connect_existing` refuses tables without them. Events already written have no tenant
and keep their fingerprints.

**The job queue has a `tenant` column.** `Queue::connect` adds it at boot. A service on
`Queue::connect_existing` runs `SETUP_STATEMENTS` again in a migration before it
deploys 0.14, as the queue now writes the column, and `check` refuses a table without
it. Each statement is safe to repeat, on Postgres, MySQL, and SQLite alike.

**The guards record the tenant on the request span.** `axa-paseto-auth` and
`axa-keycloak-auth` record the tenant they verified, but only where the span has a
`tenant` field, which Axa adds only when `HTTP_LOG_TENANT` is on. With it off, as by
default, logs are as they were.

**`DB_STATEMENT_TIMEOUT_MS` and `DB_READ_ONLY` work on MySQL.** In 0.13 production
refused either with a MySQL database. Now each MySQL connection starts with
`max_execution_time`, which stops only a read-only `SELECT`, and
`transaction_read_only`. A service that set them for Postgres and moved to MySQL gets
them. SQLite still refuses both.

**Every Postgres pool recovers from a stale plan.** In 0.13 only `axa-postgres-sqlx` did.
Now the SeaORM, Diesel, and Toasty pools retire a connection whose cached plan a
migration made stale, and `axa::Error` answers that error `503` with `Retry-After`
rather than `500`.

**`Verified` works with `AppContext`.** A webhook handler can take
`axa::webhooks::Verified` with the `Verifier` put in `ctx.shared_store`, rather than
calling `verify` itself. A service that calls `verify` need not change.
