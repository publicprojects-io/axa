# axa-keycloak

Keycloak bearer authentication and ABAC enforcement for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

A service calls this when a caller already holds a Keycloak access token.

This crate stands alone, so that it can be run and audited by itself. It depends on
neither `authentication-verifier` nor `axa-paseto`, and the code it needs from them
is copied in. The crate documentation lists each copy and its source, because a fix
to the original does not reach a copy.

It covers:

- Checking a token's signature, audience, issuer, type, and times. Only an access
  token that names a configured audience passes, so a realm may need an audience
  mapper on its clients.
- Turning Keycloak roles into ABAC attributes through an explicit role map, so an
  unmapped role grants nothing.
- Evaluating ABAC policy on each request, with a guard, middleware, and an extractor.
- Accepting PASETO tokens alongside Keycloak ones, through a verifier the service
  supplies with `Guard::with_paseto`.
- Configuration from `<PREFIX>_KEYCLOAK_*` variables.

It does not cover browser sign-in, single logout, or the Keycloak admin API.

Enable it through the `keycloak` feature of `axa`, or depend on this crate alone.
See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
