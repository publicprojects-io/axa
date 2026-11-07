# The transactional outbox

A service that commits a change and then tells a broker can crash in between, and the
event is gone. With `axa-outbox` the event is a row, written in the same transaction as
the change, and a relay publishes it afterwards. This guide is what a service writes.
The crate documentation has the rules, and section 4.17 of the
[spec](../../spec/index.md#417-outbox) has what is proved.

```toml
axa = { version = "0.6", features = ["outbox"] }
```

## Make the table

`Outbox::connect` creates `axa_outbox` at boot. Where a service's role may not change
the schema, run `axa::outbox::SETUP_STATEMENTS` in a migration, in order, and call
`Outbox::connect_existing`, which fails at boot if the table is missing.

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

## Run the relay beside the server

This is ordinary code in `main.rs`, and there is no trait to implement:

```rust,ignore
use std::time::Duration;
use axa::outbox::{Failure, Outbox, OutboxOptions};
use tokio::sync::watch;

let outbox = Outbox::connect_existing(&database_url, OutboxOptions::default()).await?;
let (stop, stopped) = watch::channel(false);
let relay = tokio::spawn(outbox.clone().run(
    move |event| {
        let producer = producer.clone();
        async move {
            let bytes = event.payload.to_string();
            let sent = producer
                .send(event.key, bytes)
                .await
                .map_err(Failure::unavailable)?;
            // Wait for the broker: this is what makes `Ok` true.
            sent.wait().await.map_err(Failure::rejected)?;
            Ok(())
        }
    },
    stopped,
));

let served = server::run(Some(router), queue, &config.server).await;
let _ = stop.send(true);
let _ = tokio::time::timeout(Duration::from_secs(5), relay).await;
served
```

The relay hears last, so that events from requests still in flight go out. A publish in
progress finishes, and the rest of the batch goes back. Giving up after the timeout is
safe: a claim left behind comes back after `stale_minutes`.

Three rules for the closure. The Fluvio lines above show the shape, and have not been
compiled here, as `axa` does not depend on a broker client.

1. **Answer `Ok` only once the broker has acknowledged the event.** A send that only
   queues locally, or a publisher that only logs, marks rows sent that nobody received,
   and the sweep then deletes the evidence.
2. **Say which failure it was.** `Failure::unavailable` means the broker is down, so no
   row is charged and the relay waits. `Failure::rejected` means this event was refused,
   so it is retried, and after `max_attempts` left as `failed`. If you cannot tell, say
   `unavailable`: a refusal called an outage holds up its own key until someone looks,
   while an outage called a refusal charges every row it reaches.
3. **Never quote a payload in an error you log.** The relay keeps your failure text on
   the row and out of its own log.

A broker client that batches can do better with its own loop: `claim`, send the batch,
flush once, and pass the rows to `sent`, or to `failed` or `release`. Anything that
claims also recovers what a dead relay left, so there is nothing else to run, but it must
call `purge_sent` itself. See `heartbeat` for a batch that takes longer than
`stale_minutes`.

## What a consumer must do

Delivery is at least once, so **dedupe on `event_id`**. A row caught between the broker
and the ack is published again, and that cannot be closed from this side.

`id` rises with each event for a key, and survives restarts, unlike a per-process
counter. Send it with the event if a consumer applies state, and refuse anything older
than the last one applied. A consumer cannot do this with a counter that restarts with
the process, as everything after a restart would look old. Order holds for one topic and key, unless a row ended as
`failed`, as the rows behind it then go ahead.

## Watching it

`Outbox::stats` says what is owed. Export `pending`, `failed`, and `oldest` as metrics.
A rising `oldest` means rows are not going out, and any `failed` needs a person:

```sql
SELECT id, topic, kind, attempts, error FROM axa_outbox WHERE status = 'failed';
```

Once the cause is fixed, `Outbox::retry_failed` puts them back. They may then arrive
after later events for their key. Do not fail readiness on a backlog: a broker outage
should back the table up, not take the service down.

## Personal data

A payload is plain JSONB, so a snapshot of a person is a second copy of them.

- Prefer an id to a snapshot where consumers can look the record up.
- `retention_days`, 7 by default, sweeps sent rows. Set it as low as your operators can
  live with: zero sweeps them at the next round.
- A `failed` row is never swept, as it is still owed, so its payload stays until someone
  deals with it. Include `axa_outbox` in an erasure:
  `DELETE FROM axa_outbox WHERE key = $1`.
- Never put a secret in a payload.

## Moving from a service's own `event_outbox`

Stop the old relay, then in a migration:

```sql
INSERT INTO axa_outbox (event_id, topic, kind, key, payload, created_at)
SELECT event_id, entity, kind, entity_pid::text, payload, occurred_at
FROM event_outbox
WHERE published_at IS NULL
ORDER BY id;
```

Then:

1. Replace `OutboxInsert::insert_on` with `write` or `write_seaorm`. The envelope stays
   the payload, so consumers see no change.
2. Replace `relay.rs` with the closure above, keeping the send from the `EventSink`.
   Say which failure each error is, and wait for the broker's acknowledgement.
3. Replace `/events/recent`'s read with a query on `axa_outbox`. A read must not lock.
4. A second reader of the table, such as a webhook fan-out, moves inside the closure,
   after the primary send. It runs at least once, so it needs its own unique key, as a
   `webhook_deliveries` row has.
5. Drop `event_outbox` once it has been empty of unpublished rows for a release.
