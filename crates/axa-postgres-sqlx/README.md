# axa-postgres-sqlx

PostgreSQL pooling, migrations, and test resets over sqlx for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

SQL that cannot run in a transaction, such as `CREATE INDEX CONCURRENTLY`, runs as a
`Background` migration through `run_background`, one process at a time under an
advisory lock, and each is recorded in `axa_background_migrations`.

Enable it through the `postgres-sqlx` feature of `axa`, or depend on this crate alone. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
