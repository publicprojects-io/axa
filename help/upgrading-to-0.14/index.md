# Upgrading to Axa 0.14

This covers a service on 0.13, and is written as 0.14 is built. Most services upgrade by
changing the version. A service that builds a configuration struct field by field adds
the new fields.

## What will not compile

| Change | What to do |
|---|---|
| `config::Http` has `log_tenant` | Add `log_tenant: false` where you build an `Http` by hand |

## What changes with no compile error

**The guards record the tenant on the request span.** `axa-paseto-auth` and
`axa-keycloak-auth` record the tenant they verified, but only where the span has a
`tenant` field, which Axa adds only when `HTTP_LOG_TENANT` is on. With it off, as by
default, logs are as they were.

**`Verified` works with `AppContext`.** A webhook handler can take
`axa::webhooks::Verified` with the `Verifier` put in `ctx.shared_store`, rather than
calling `verify` itself. A service that calls `verify` need not change.
