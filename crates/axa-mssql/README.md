# axa-mssql

SQL Server pooling, migrations, and test resets for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

It has the same shape as the other Axa database crates: a `Pool`, a `PoolConfig`,
`connect`, `ping`, `migrate`, `pending`, `revert`, `truncate`,
`dangerously_recreate`, `is_unique_violation`, and `quote_identifier`. It uses the
[`mssql`](https://crates.io/crates/mssql) driver, a maintained fork of Tiberius, over
Tokio and a `deadpool` pool, with TLS through `rustls` and `ring`, so there is no
ODBC, no OpenSSL, and no C in the database path.

A connection string is ADO.NET, as SQL Server and Azure hand out, or JDBC. Migrations
are a list of T-SQL migrations in code, each run in a transaction under a lock, so
services booting together take turns. Each connection gets `XACT_ABORT ON` and, when
asked, a `LOCK_TIMEOUT`, and a connection goes back into use with any transaction a
caller left open rolled back.

Enable it through the `mssql` feature of `axa`, which answers its errors as `409`,
`422`, or `503` as for the other databases, or depend on this crate alone. It is not
one of `axa`'s database features: it adds no `axa::model` and is not
`AppContext::db`. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
