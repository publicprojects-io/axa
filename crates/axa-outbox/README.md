# axa-outbox

A transactional outbox and relay on Postgres for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

An event is written in the same transaction as the change it describes, and a relay publishes it afterwards, so a crash between the commit and the publish loses nothing. Delivery is at least once, so a consumer must dedupe on the event's id, which the inbox here does in the consumer's transaction. The broker client stays in the service: the relay takes closures, or the service's own loop can claim and record batches.

The outbox keeps a record. Events and attempts are only ever inserted, and the database refuses every change, so what a service delivered, refused, and failed stays on record, and nothing is swept. A service's decision runs before the publisher, in the claim's transaction, and can deliver, refuse for good, or hold at no cost. Every outcome commits with the service's audit row. A payload is erased only by a recorded act, a changed row is never published, and with the `encryption` feature payloads are sealed at rest.

Version 0.2.0 replaces the 0.1.0 table, and imports its rows: see [Upgrading axa-outbox to 0.2](https://github.com/publicprojects-io/axa/blob/main/help/upgrading-to-0.3/index.md#upgrading-axa-outbox-to-02).

Enable it through the `outbox` feature of `axa`, or depend on this crate alone. See the crate documentation for usage, and the [guide](https://github.com/publicprojects-io/axa/blob/main/help/transactional-outbox/index.md) for what a service changes.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
