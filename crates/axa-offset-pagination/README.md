# axa-offset-pagination

Offset pagination, count headers, and `LIKE` escaping for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

**This crate is insecure. Prefer [axa-cursor-pagination](https://crates.io/crates/axa-cursor-pagination).** It stays for services that already page this way, and it will not gain features:

- **A total can leak.** `X-Total-Count` can count rows the caller's record policy hides, so a caller learns that they exist.
- **An offset is a scan.** The database reads and discards every row before the offset, up to the maximum, on each request.
- **The position is the caller's.** Any caller can ask for any offset up to the maximum, and nothing ties it to the query it came from.

This was published as `axa-pagination`, whose 0.1.1 it supersedes. Enable it through the `offset-pagination` feature of `axa`, or the old `pagination` feature, which will go.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
