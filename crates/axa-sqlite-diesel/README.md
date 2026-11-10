# axa-sqlite-diesel

Diesel pooling, migrations, and test resets for Axum services on SQLite, from the [axa](https://github.com/publicprojects-io/axa) family.

It has the same shape as `axa-postgres-diesel`. Each pooled connection is a
`diesel-async` `SyncConnectionWrapper` around Diesel's `SqliteConnection`, which
runs queries on Tokio's blocking threads. SQLite is compiled into the binary from
`libsqlite3-sys` with its `bundled` feature, at the version `sqlx` pins, so
nothing is needed on the host. URLs are `sqlite://path` or `sqlite::memory:`, and
every connection turns on foreign keys, a busy timeout, and, for a file, WAL.
There is no network, so there is no TLS.

Enable it through the `sqlite-diesel` feature of `axa`, which makes its `Pool`
the service's `AppContext::db`, or depend on this crate alone.
See the crate documentation for usage.

## Not yet published

This crate is held back from crates.io. `libsqlite3-sys` 0.37, the version `sqlx`
0.9 pins, has the advisory
[RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html), and
this crate will be published once `sqlx` releases with a fixed version.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
