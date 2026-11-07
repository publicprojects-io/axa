# axa-outbox

A transactional outbox and relay on Postgres for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

An event is written in the same transaction as the change it describes, and a relay publishes it afterwards, so a crash between the commit and the publish loses nothing. Delivery is at least once, so a consumer must dedupe on the event's id. The broker client stays in the service: the relay takes a closure, or the service's own loop can claim and acknowledge batches.

Enable it through the `outbox` feature of `axa`, or depend on this crate alone. See the crate documentation for usage, and the [guide](https://github.com/publicprojects-io/axa/blob/main/help/transactional-outbox/index.md) for what a service changes.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
