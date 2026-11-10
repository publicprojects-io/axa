# axa-mysql-sqlx

MySQL pooling, migrations, and test resets over sqlx for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

Enable it through the `mysql-sqlx` feature of `axa`, which makes its `Pool` the service's `AppContext::db`, or depend on this crate alone. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
