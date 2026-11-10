# axa-mysql-diesel

Diesel pooling, migrations, and test resets for Axum services on MySQL, from the [axa](https://github.com/publicprojects-io/axa) family.

It has the same shape as `axa-postgres-diesel`. It uses `diesel-async` over
`mysql_async`, so there is no `libmysqlclient` to link, no OpenSSL, and no C in
the database path. Connections use TLS through `rustls` with the standard web
roots, as the connection URL's `ssl-mode` says, the same parameter `sqlx` reads,
so one URL works with every axa MySQL crate.

Enable it through the `mysql-diesel` feature of `axa`, which makes its `Pool` the
service's `AppContext::db`, or depend on this crate alone. See the crate
documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
