# axa-redis

A Redis cache with mandatory expiry for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

Every entry expires, and values are stored as JSON under a key prefix. A cache
failure never has to take a service down: `get_or_load` falls back to the real
source.

Enable it through the `redis` feature of `axa`, or depend on this crate alone.
See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
