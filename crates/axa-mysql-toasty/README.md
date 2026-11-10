# axa-mysql-toasty

Toasty connections, migrations, and test resets for Axum services, on MySQL, from the [axa](https://github.com/publicprojects-io/axa) family.

It has the same shape as `axa-postgres-toasty` and the other Axa database crates: a `Pool`, a `PoolConfig`, `connect`, `ping`, `migrate`, `pending`, `truncate`, `dangerously_recreate`, `is_unique_violation`, and `quote_identifier`. Toasty migrations go forward only, so there is no `revert`.

URLs are `mysql://user:password@host:port/database`, and TLS is asked for with the `ssl-mode` query parameter, as with every Axa MySQL crate. `truncate` and `dangerously_recreate` turn foreign key checks off on one connection while they work, and back on after. `error_number` and `sql_state` give the MySQL error number and `SQLSTATE` of a database error, except for a deadlock or a write in a read-only transaction, which Toasty's driver reports as its own kinds of error.

Toasty is before 1.0 and changes often, so expect this crate to follow its breaking releases.

Enable it through the `mysql-toasty` feature of `axa`, which makes its `Pool` the service's `AppContext::db`, or depend on this crate alone. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
