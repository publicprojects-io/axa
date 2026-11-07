# axa-idempotency

`Idempotency-Key` handling over Postgres for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family: a retried write gets its first answer back instead of running twice.

Enable it through the `idempotency` feature of `axa`, or depend on this crate alone. See the crate documentation for usage, and [where the layer goes](https://github.com/publicprojects-io/axa/blob/main/help/idempotency-keys/index.md).

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
