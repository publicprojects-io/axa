# Upgrading to Axa 0.14

This covers a service on 0.13, and is written as 0.14 is built. Most services upgrade by
changing the version. A service that builds a configuration struct field by field adds
the new fields.

## What will not compile

| Change | What to do |
|---|---|
| `config::Http` has `log_tenant` | Add `log_tenant: false` where you build an `Http` by hand |
| `JobInfo` has `tenant`, and `SETUP_STATEMENTS` in each job queue has one more statement | Nothing, unless you match on the array's length |

## What changes with no compile error

**The job queue has a `tenant` column.** `Queue::connect` adds it at boot. A service on
`Queue::connect_existing` runs `SETUP_STATEMENTS` again in a migration before it
deploys 0.14, as the queue now writes the column, and `check` refuses a table without
it. Each statement is safe to repeat, on Postgres, MySQL, and SQLite alike.

**The guards record the tenant on the request span.** `axa-paseto-auth` and
`axa-keycloak-auth` record the tenant they verified, but only where the span has a
`tenant` field, which Axa adds only when `HTTP_LOG_TENANT` is on. With it off, as by
default, logs are as they were.

**`Verified` works with `AppContext`.** A webhook handler can take
`axa::webhooks::Verified` with the `Verifier` put in `ctx.shared_store`, rather than
calling `verify` itself. A service that calls `verify` need not change.
