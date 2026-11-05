# axa-diesel

Diesel pooling, migrations, and test resets for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

It uses `diesel-async`, so there is no `libpq` to link and no C in the database
path. Connections use TLS with the standard web roots unless the connection
string says `sslmode=disable`.

Enable it through the `diesel` feature of `axa`, or depend on this crate alone.
See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
