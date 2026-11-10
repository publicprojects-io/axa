# axa-sqlite-sqlx

SQLite pooling, migrations, and test resets over sqlx for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family. It is the SQLite twin of `axa-postgres-sqlx`, with the same shape.

Every connection has foreign keys on, a busy timeout, and WAL for a file. URLs are `sqlite://path` or `sqlite::memory:`, and a missing file is created only when the URL says `?mode=rwc`.

Enable it through the `sqlite-sqlx` feature of `axa`, which makes its `Pool` the service's `AppContext::db`, or depend on this crate alone. See the crate documentation for usage.

This crate is held from publishing for now. sqlx 0.9 pins `libsqlite3-sys` 0.37, whose bundled C SQLite has advisory RUSTSEC-2026-0459, until sqlx releases a raised cap on it.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
