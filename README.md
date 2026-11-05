# axa

Small shared library for services built directly on [Axum](https://docs.rs/axum)
and [sqlx](https://docs.rs/sqlx).

This is a library, not a framework: a service's `main.rs` stays ordinary Axum
and calls into this crate. There are no lifecycle hooks, application context
types, or macros.

The crate is a new scaffold and has no public API yet.

## License

See [LICENSE.md](LICENSE.md).
