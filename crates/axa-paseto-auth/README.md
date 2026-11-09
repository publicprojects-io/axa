# axa-paseto-auth

PASETO bearer authentication and ABAC enforcement for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

Signature checks and policy evaluation live in
[`authentication-verifier`](https://crates.io/crates/authentication-verifier).
This crate adds the Axum layer on top: the guard, the action derivation, and the
extractor.

Enable it through the `paseto` feature of `axa`, or depend on this crate alone.
See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
