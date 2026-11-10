# axa-toasty

Toasty connections, migrations, and test resets for Axum services, on Postgres, from the [axa](https://github.com/publicprojects-io/axa) family.

It has the same shape as `axa-sqlx`, `axa-seaorm`, and `axa-diesel`: a `Pool`, a `PoolConfig`, `connect`, `ping`, `migrate`, `pending`, `truncate`, `dangerously_recreate`, `is_unique_violation`, and `quote_identifier`. Toasty migrations go forward only, so there is no `revert`.

Toasty is before 1.0 and changes often, so expect this crate to follow its breaking releases. Its Postgres driver brings in `rustls-pemfile`, which RustSec lists as unmaintained (RUSTSEC-2025-0134). That is a notice, not a vulnerability.

Enable it through the `toasty` feature of `axa`, which makes its `Pool` the service's `AppContext::db`, or depend on this crate alone. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
