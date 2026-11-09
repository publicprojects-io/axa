# axa-request-rate-limit

In-process request limits and failed-login lockout for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

Enable it through the `ratelimit` feature of `axa`, or depend on this crate alone. See the crate documentation for usage.

Counting by a key of the service's own (`by_key`), the choice for a full table
(`WhenFull`), the first refusal in a window, the verdict on each response, and a
limit per key (`set_limit_for`), and the warning about one proxy were added for one service so far, the electronic
patient record service. No second service uses them yet.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
