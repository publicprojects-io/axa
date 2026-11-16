# axa-api-tokens

Opaque API tokens for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

A token reads `crm_<id>_<secret>`. The prefix says whose it is, for secret scanners. The
id finds its row, and only the SHA-256 hash of the secret is kept, so a copy of the table
lets no one in. Each record has scopes, an optional expiry, a revoked flag, and the time
of its last use.

Enable it through the `api-tokens` feature of `axa`, or depend on this crate alone. See
the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
