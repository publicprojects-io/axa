# axa-request-rate-limit

In-process request limits and failed-login lockout for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

Enable it through the `ratelimit` feature of `axa`, or depend on this crate alone. See the crate documentation for usage.

Counting by a key of the service's own (`by_key`), the choice for a full table
(`WhenFull`), the first refusal in a window, the verdict on each response, and a
limit per key (`set_limit_for`), and the warning about one proxy were added for one service so far, the electronic
patient record service. No second service uses them yet.

## Across replicas

With the `postgres` or `redis` feature, `shared::SharedLimiter` counts in a store every
replica shares, in one statement per request, so three replicas give a client one
limit, not three. The store holds only a hash of each client keyed by the service's
secret. `WhenDown` makes the service say what an outage of the store does: let requests
through, or refuse them with a `503`.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
