# axa-mysql-seaorm

SeaORM connections, migrations, schema helpers, and test resets for Axum services on
MySQL, from the [axa](https://github.com/publicprojects-io/axa) family.

It has the same shape as `axa-postgres-seaorm` and the other Axa database crates: a
`Pool`, a `PoolConfig`, `connect`, `ping`, `migrate`, `revert`, `pending`, `truncate`,
`dangerously_recreate`, `is_unique_violation`, and `quote_identifier`. The migration
functions take the service's `Migrator` as a type. It keeps Loco's helpers too:
`query::condition()`, `schema`, and `truncate_table`.

What differs for MySQL:

- The URL is `mysql://user:password@host:port/database`. TLS is asked for with
  `?ssl-mode=`, one of `DISABLED`, `PREFERRED`, `REQUIRED`, `VERIFY_CA`, or
  `VERIFY_IDENTITY`, through rustls.
- `quote_identifier` uses backticks.
- `truncate` empties each table with foreign key checks off, and resets its
  `AUTO_INCREMENT`. MySQL has no `CASCADE`, so name the tables that refer to it too.
- `dangerously_recreate` drops every table in `DATABASE()` with foreign key checks
  off, then applies every migration.
- MySQL commits each DDL statement as it goes, so a migration that fails part way
  stays part applied.
- In `schema`, a timestamp is a MySQL `DATETIME` in UTC, not a `TIMESTAMP`, which ends
  in 2038. JSON is `JSON`, and a UUID is `BINARY(16)`.

Enable it through the `mysql-seaorm` feature of `axa`, which makes its `Pool` the
service's `AppContext::db`, or depend on this crate alone.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
