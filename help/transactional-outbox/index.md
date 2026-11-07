# The transactional outbox

A service that commits a change and then tells a broker can crash in between, and the
event is gone. With `axa-outbox` the event is a row, written in the same transaction as
the change, and a relay publishes it afterwards. This guide is what a service writes.
The crate documentation has the rules, and section 4.17 of the
[spec](../../spec/index.md#417-outbox) has what is proved.

```toml
axa = { version = "0.7", features = ["outbox"] }
# Or, to seal payloads at rest:
axa = { version = "0.7", features = ["outbox-encryption"] }
```

`axa-outbox` 0.2 keeps a **record**. What a service delivered, what it refused, and what
failed are evidence it may have to answer for, so they are only ever added to. Coming
from 0.1, read [Upgrading axa-outbox to 0.2](../upgrading-to-0.3/index.md#upgrading-axa-outbox-to-02).

## Make the tables

`Outbox::connect` creates them at boot. Where a service's role may not change the
schema, run `axa::outbox::SETUP_STATEMENTS` in a migration, in order, and call
`Outbox::connect_existing`, which refuses to start if a table, an index, or a trigger is
missing or turned off. They need Postgres 14 or later.

| Table | Is | Holds |
|---|---|---|
| `axa_outbox_events` | A record | Each event, without its payload, and its fingerprint |
| `axa_outbox_payloads` | A record | Each payload. The one change allowed is an erasure |
| `axa_outbox_attempts` | A record | Everything that happened to each event, naming the relay and the claim |
| `axa_outbox_owed` | Working state | What is still owed, kept in step with the attempts by a trigger, and holds. It can be made again from the record |
| `axa_outbox_leases` | Working state | When each live claim last reported in |

`axa_outbox_owed` is derived from the record, and its guard refuses any change that
does not come from the record's own triggers, apart from a hold, which it lets move no
more than a day ahead. So deleting a row there cannot quietly stop an event. The guard
tells the record's triggers apart from a plain statement by `pg_trigger_depth()`, so a
write made from inside any other trigger also passes. Making a trigger needs rights over
a table, so keep the service's role from creating triggers, as it should not change the
schema anyway. If it ever
no longer matches the record, as after someone with the schema turned the guard off,
make it again: with the relays stopped, run `axa::outbox::REBUILD_OWED_STATEMENTS` in
one transaction, as the tables' owner. They read the whole record and change nothing
in it, and holds are decided again. `axa_outbox_leases` refuses a lease dated ahead or
moved to another claim. Changing or deleting one can at worst bring its claim's
recovery early, and the recovery is on record.

The records' triggers refuse `UPDATE`, `DELETE`, and `TRUNCATE` from any role, and they
fire even with `session_replication_role` set to `replica`. A role that owns the tables
can still drop them or turn a trigger off, so the service's own role should not own
them. It needs `SELECT` and `INSERT` on the records, `UPDATE` on
`axa_outbox_payloads` if it erases, and all of `SELECT`, `INSERT`, `UPDATE`, and
`DELETE` on the working state, which the guards still hold to the rules above.

## Write the event in the transaction

```rust,ignore
use axa::outbox::{Event, write};

let mut transaction = pool.begin().await?;
sqlx::query("UPDATE people SET name = $2 WHERE pid = $1")
    .bind(pid)
    .bind(&name)
    .execute(&mut *transaction)
    .await?;
let event = Event::new("person", "updated", pid.to_string(), payload);
write(&mut transaction, &event).await?;
transaction.commit().await?;
```

With SeaORM, `write_seaorm(&transaction, &event)` takes a `DatabaseTransaction`. Neither
takes a pool, because an event written outside the transaction is the bug this exists
to prevent.

- `topic` is what the event is about, and `kind` is what happened.
- `key` keeps events in order, and is usually the record's public id.
- `Event::new` makes an `event_id`. A service with its own envelope id sets that field.
- Change the record **before** writing the event. Its row lock is what makes two
  transactions on one record write their events in commit order.
- A payload over 64 KiB fails the write, and the caller's transaction with it.

For anything else, build one `Writer` at boot, keep it in the service's state, and call
its `write` or `write_seaorm`:

```rust,ignore
use axa::outbox::{Key, Keys, WriteOptions, Writer};

let writer = Writer::new(WriteOptions {
    max_payload_bytes: 16 * 1024,
    deny: vec!["nhs_number".into(), "date_of_birth".into()],
})?
// With `outbox-encryption`. The secret is at least 32 random bytes, from the service's
// own secret store, never from the code.
.sealing(Keys::new(Key::new(1, &secret)?));

writer.write(&mut transaction, &event).await?;
```

A field in `deny` fails the write wherever it is in the payload, as an object key at any
depth. The list is empty unless a service fills it.

## Run the relay beside the server

This is ordinary code in `main.rs`, and there is no trait to implement:

```rust,ignore
use std::time::Duration;
use axa::outbox::{Decision, Failure, Outbox, OutboxOptions};
use tokio::sync::watch;

let outbox = Outbox::connect_existing(&database_url, OutboxOptions::default()).await?;
let (stop, stopped) = watch::channel(false);
let relay = tokio::spawn(outbox.clone().run(
    // Decide, in the claim's transaction, with the claimed rows locked.
    async |event, tx| {
        if consent::withdrawn(tx, &event.key).await? {
            return Ok(Decision::refuse("consent_withdrawn"));
        }
        if death::reported(tx, &event.key).await? {
            return Ok(Decision::Hold);
        }
        Ok(Decision::Deliver)
    },
    // Publish.
    move |event| {
        let producer = producer.clone();
        async move {
            let bytes = event.payload.to_string();
            let sent = producer
                .send(event.key, bytes)
                .await
                .map_err(|_| Failure::unavailable("broker_unreachable"))?;
            // Wait for the broker: this is what makes `Ok` true.
            sent.wait().await.map_err(|_| Failure::rejected("broker_rejected"))?;
            Ok(())
        }
    },
    // Record, in the outcome's transaction, before it commits.
    async |attempt, tx| audit::outbox(tx, attempt).await,
    stopped,
));

let served = server::run(Some(router), queue, &config.server).await;
let _ = stop.send(true);
let _ = tokio::time::timeout(Duration::from_secs(5), relay).await;
served
```

A service with no rule passes `axa::outbox::decide::deliver_all`, and one with nothing to
audit passes `axa::outbox::record::nothing`. Both are required, so that the choice is in
the service's own code and visible in review.

The relay hears last, so that events from requests still in flight go out. A publish in
progress finishes, and the rest of the batch goes back. Giving up after the timeout is
safe: a claim left behind is recovered after `stale_minutes`.

### The decision

- **`Deliver`** sends the event.
- **`Refuse(reason)`** never sends it. That is final: nothing puts it back, and
  `release` refuses it. The refusal and its audit row are written in the claim's
  transaction.
- **`Hold`** writes nothing and charges nothing. The event is decided again after
  `hold_interval`, 10 seconds by default, for as long as the service holds it, and it
  holds back its own key and no other. A hold has no limit: watch `held` and
  `oldest_held` in `stats`.

A decision that fails, with a database error say, leaves its event as it was, to be
decided again after `hold_interval`, and is logged.

### The publisher

The Fluvio lines above show the shape, and have not been compiled here, as `axa` does
not depend on a broker client.

1. **Answer `Ok` only once the broker has acknowledged the event.** A send that only
   queues locally, or a publisher that only logs, records deliveries nobody received.
2. **Say which failure it was.** `Failure::unavailable` means the broker is down, so no
   event is charged and the relay waits. `Failure::rejected` means this event was
   refused, so it is retried, and after `max_attempts` left as `failed`. If you cannot
   tell, say `unavailable`: a refusal called an outage holds up its own key until
   someone looks, while an outage called a refusal charges every event it reaches.
3. **Give a code, not the broker's text.** A reason is at most 200 characters with no
   line breaks, and is kept on the attempt. A broker's error can quote what it was sent,
   so log it yourself if you need it, and never with the payload.

### The record

`record` runs after each outcome is written, `delivered`, `refused`, `rejected`,
`failed`, `lost`, or `abandoned`, in the same transaction, before it commits. Write the
service's audit row there. If it fails, the outcome rolls back with it: the event stays
claimed, to be recovered after `stale_minutes`, and the rest of the batch goes back. So
an outcome is never on record without its audit, nor the audit without the outcome.

### Its own loop

A broker client that batches can do better with its own loop: `claim(decide, record)`,
send the batch, flush once, and then, in one transaction with its audit rows, pass the
events to `delivered`, or to `failed`, `refused`, or `returned`. Each takes the
service's transaction, and each answers the attempt it wrote, or nothing if the claim
was given up on. Anything that claims also recovers what a dead relay left, so there is
nothing else to run. See `heartbeat` for a batch that takes longer than `stale_minutes`.

## What a consumer must do

Delivery is at least once, so **dedupe on `event_id`**. An event caught between the
broker and the ack is published again, and that cannot be closed from this side. The
inbox does it in the consumer's transaction:

```rust,ignore
let mut transaction = pool.begin().await?;
if axa::outbox::inbox::seen(&mut transaction, event_id).await? {
    return Ok(()); // Applied before.
}
apply(&mut transaction, &event).await?;
transaction.commit().await?;
```

The consumer's migration runs `axa::outbox::inbox::SETUP_STATEMENTS`. `axa_inbox` is a
record too, and refuses every change. A note rolled back with the transaction is not
seen, so an event whose handling failed is applied when it comes again.

`id` rises with each event for a key, and survives restarts, unlike a per-process
counter. Send it with the event if a consumer applies state, and refuse anything older
than the last one applied. Order holds for one topic and key, unless an event ended as
`failed`, as the events behind it then go ahead. A topic where one stuck key must not
hold back the rest can be given `Order::None` in `OutboxOptions::topic_order`.

## Watching it

`Outbox::stats` says what is owed. Export `pending`, `held`, `failed`, `tampered`, and
`oldest` as metrics, with `delivered` and `refused` for the last hour. A rising `oldest`
means events are not going out, a rising `oldest_held` is a hold nobody lifted, and any
`failed` or `tampered` needs a person:

```sql
SELECT e.id, e.topic, e.kind, o.charges, o.reason
FROM axa_outbox_owed o JOIN axa_outbox_events e USING (event_id)
WHERE o.state = 'failed';
```

Once the cause is fixed, put one back with `release`, which records who and why in the
caller's transaction, beside the service's own audit row:

```rust,ignore
axa::outbox::release(&mut transaction, event_id, "ops@example.com", "broker_fixed").await?;
```

It may then arrive after later events for its key. A refused, delivered, or erased event
cannot be released. Do not fail readiness on a backlog: a broker outage should back the
table up, not take the service down.

A `tampered` event no longer matched the fingerprint written with it, so someone changed
it with the triggers off. It is never published. Find out who before releasing it.

## Personal data

A snapshot of a person in a payload is a second copy of them, kept as long as the
record is, which is for good.

- **Prefer an id to a snapshot** where consumers can look the record up. The 64 KiB
  limit is a backstop, not the advice.
- Name the fields that must never leave in `WriteOptions::deny`.
- With `outbox-encryption`, seal payloads at rest. Give each new secret a new key id,
  and keep the old key in `Keys::and_open_with` until no event sealed under it is owed.
  A relay needs the same keys, through `Outbox::opening`. One without them leaves sealed
  events where they are, and logs that it is stuck.
- To erase a person, erase each of their events' payloads with `erase`, which appends an
  `erased` attempt naming who and why, then blanks the payload, in the caller's
  transaction. An erased event is never published. Its topic, kind, key, and
  fingerprint stay. On a plain payload the fingerprint can confirm a guess of what was
  erased, which a sealed one cannot, as it covers only the ciphertext.
- Never put a secret in a payload.

## Moving from a service's own `event_outbox`

Stop the old relay, run `SETUP_STATEMENTS`, then in a migration:

```sql
WITH moved AS (
    INSERT INTO axa_outbox_events (event_id, topic, kind, key, fingerprint, created_at)
    SELECT event_id, entity, kind, entity_pid::text,
        axa_outbox_fingerprint(event_id, entity, kind, entity_pid::text, payload, NULL),
        occurred_at
    FROM event_outbox
    WHERE published_at IS NULL
    ORDER BY id
    RETURNING event_id
)
INSERT INTO axa_outbox_payloads (event_id, payload)
SELECT moved.event_id, old.payload
FROM moved JOIN event_outbox old ON old.event_id = moved.event_id;
```

Then:

1. Replace `OutboxInsert::insert_on` with `write` or `write_seaorm`. The envelope stays
   the payload, so consumers see no change.
2. Replace `relay.rs` with the closures above, keeping the send from the `EventSink`.
   Say which failure each error is, with a code, and wait for the broker's
   acknowledgement. A policy check the old relay made goes in the decision.
3. Replace `/events/recent`'s read with a query on `axa_outbox_events` and
   `axa_outbox_attempts`. A read must not lock.
4. A second reader of the table, such as a webhook fan-out, moves inside the publisher,
   after the primary send. It runs at least once, so it needs its own unique key, as a
   `webhook_deliveries` row has.
5. Drop `event_outbox` once it has been empty of unpublished rows for a release.
