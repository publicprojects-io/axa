# axa-environment

Which environment a service runs in, from the [axa](https://github.com/publicprojects-io/axa) family.

It has no dependencies, so a migration crate can use it without depending on `axa`. `axa` re-exports it, so a safety guard in a migration and the service itself read the environment the same way. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
