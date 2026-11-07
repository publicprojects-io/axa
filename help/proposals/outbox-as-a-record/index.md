# Proposal: an outbox that keeps a record

**Status:** accepted, and built for `axa-outbox` 0.2.0, with the spec changes in
sections 3, 4.17, and 6. It differs from what is below in two ways. Payloads live in
their own table, `axa_outbox_payloads`, whose trigger allows one change, the blanking of
a payload once the event has an `erased` attempt, so there is no `SECURITY DEFINER`
function. And payloads can be sealed at rest, with XChaCha20-Poly1305, behind the
`encryption` feature, though "Out of scope" below said not. The open questions were
settled as: a hold has no limit and changes no state, and `stats` shows it; heartbeats
go to a mutable `axa_outbox_leases`, and a claim and its recovery are attempts; and the
`axa-idempotency`, `axa-worker`, and `axa-offset-pagination` changes are each owed as a
later change of their own, in section 6 of the spec.

**Breaking:** yes. `axa-outbox` 0.2.0 replaces the 0.1.0 table and its behaviour. The
0.1.0 shape is not kept beside it as an option or a default.

**From:** the electronic-patient-record service, which runs its own outbox (its task
OS-3) and could not adopt `axa-outbox` 0.1.0 without weakening what its clinical safety
case records.

## Summary

`axa-outbox` 0.1.0 treats an event as **state**: one mutable row that moves from
`pending` to `claimed` to `sent` or `failed`, and is deleted once sent. That loses what a
service must be able to answer for: what it disclosed, what it refused, and when. In
0.2.0 an event is a **record**:

- events and attempts are only ever inserted, and the database refuses every change;
- a refusal is final;
- a service can hold an event without spending its attempts;
- every outcome commits together with the service's audit row;
- nothing is swept, and a payload is erased only by a recorded act.

One shape, the safer one, with no switch back. A weaker behaviour kept for
compatibility is a weaker behaviour someone ships, so the proposal also adds a principle
that says so (P-10), and lists the other crates where it applies.

## Why: what fails in 0.1.0

The service's outbox keeps `outbox_events` and `outbox_attempts`. Database triggers
refuse `UPDATE`, `DELETE` and `TRUNCATE` on both. Its relay runs the service's own
read-path decision (policy, consent, and whether a report of death holds the patient)
for the recipient, and only then delivers. Against `axa-outbox` 0.1.0:

| The service needs | 0.1.0 does | Where |
|---|---|---|
| Every delivery and every refusal stays on record | A row's `status`, `attempts` and `error` are rewritten in place, and a sent row is deleted after `retention_days` | OBX-5, OBX-7, OBX-13 |
| A refusal by policy or consent is final | A row ends `sent` or `failed`. A refusal would read as one of them, and `retry_failed` would put a refused row back to be sent | OBX-7 |
| An event about a patient with a standing report of death is held, for as long as the report stands, at no cost | A publisher can charge a row an attempt (`Rejected`) or stop the whole round (`Unavailable`). A long hold ends as `failed`, and an outage reading stalls every key | OBX-9, OBX-10 |
| The outcome and its audit row commit together, or neither does | `sent` and `failed` run in the crate's own transaction, on its pool | OBX-19 |
| A changed row is never sent | A row is published as it reads at claim time | — |

These are not one service's needs. Any service that sends personal data to another party
has to say what it sent and what it refused, and 0.1.0 deletes the answer.

## Principles

Section 3 has six principles about how the library is built. None says what it promises
about the data it keeps, or which way a default leans. Proposed text:

| Id | Principle |
|---|---|
| P-2 (amended) | **Two consumers first, or one with a safety case.** Code is added only once two services need it, or once one service needs it to keep a control that a hazard log, a regulator, or a security review holds it to. A subcrate built ahead of that is marked in the README as having no service yet, and that mark comes off with its first service. |
| P-7 (new) | **A record is not state.** Where a row is evidence of something a service is answerable for, such as a disclosure, a refusal, or a decision, it is appended and never edited or deleted, and the database is given triggers that refuse it. State that is only a working copy, such as a cache, a lease, or a count, may change. A crate says which of its tables is which. |
| P-8 (new) | **Final means final.** An outcome a service has called final is never undone by a helper. Putting work back is an act of its own, by a named person, and is kept as a record. |
| P-9 (new) | **An outcome commits with its audit.** An API that records an outcome takes the caller's transaction, as `outbox::write` already does, so that a service's audit row and the outcome commit together or not at all. |
| P-10 (new) | **The safe way is the only way.** When a safer or more reliable behaviour replaces a weaker one, the weaker one goes, in a breaking release, with an upgrade path. It is not kept as an option or a default. Where a service must choose, it chooses by passing something explicit, never by leaving a default alone. |

### What the new principles reach beyond the outbox

P-10 is not only about this crate. Applied across the workspace today:

| Crate | Default that P-7 to P-10 would change | Proposed |
|---|---|---|
| `axa-outbox` | Mutable rows, swept, no final refusal, outcomes outside the caller's transaction | This proposal |
| `axa-idempotency` | `lease` defaults to one minute, so the same request can take over a key whose first run wrote and then crashed or could not keep its answer, and run the write a second time | Default the lease to the key's `ttl`. A shorter lease becomes something a service passes, knowing what it allows. A follow-up proposal |
| `axa-worker` | A person's `cancel` and `retry` change the job row in place, and finished and cancelled rows are deleted after `retention_days`. Job rows are state, but a person's act on a job is a decision (P-8) | Keep job rows as state, and append a person's `cancel`, `retry` and release of `needs_review` to an insert-only `axa_job_acts` naming who and why. A follow-up proposal |
| `axa-offset-pagination` | Kept beside `axa-cursor-pagination`, though its own documentation calls it insecure | Remove it in the next breaking release, with `axa::pagination` and the `pagination` feature. A service still paging by offset keeps 0.7 until it moves |
| `axa-ratelimit` | `WhenFull::Share` | None. Sharing is the cautious choice for most services, and one where a refusal can harm someone passes `WhenFull::Allow` explicitly, as P-10 asks |

## What changes in `axa-outbox` 0.2.0

### Requirements that change

| Id | Now | In 0.2.0 |
|---|---|---|
| OBX-5 | A lost claim comes back as `pending` by rewriting the row | A lost claim is found from its attempts: a `claimed` attempt with no later one, older than `stale_minutes`. Recovery appends an attempt and edits nothing |
| OBX-7 | A refused row ends as `failed` and `retry_failed` puts it back | A broker's refusal is still `failed` after `max_attempts`, and putting it back is `release(by, reason)`, an attempt naming who. A refusal by the service's decision is `refused`, and nothing puts it back (OBX-25) |
| OBX-10 | 1024 characters of error text are kept | A reason is a code of at most 200 characters with no line breaks (OBX-32). Free error text is logged as before, never stored |
| OBX-13 | Sent rows are deleted after `retention_days` | Removed. Nothing is swept (OBX-30) |
| OBX-19 | A service's own loop calls `sent`, `failed`, `release` on the crate's pool | Each takes the caller's transaction (OBX-28) |

The others hold as they are: the write in the caller's transaction (OBX-1), the claim
with `SKIP LOCKED` (OBX-4), late outcomes that change nothing (OBX-6), order per key
(OBX-8), outages that charge nothing (OBX-9), panics, timeouts and shutdown (OBX-11,
OBX-12, OBX-17), logs with no payload (OBX-14), and no broker client (OBX-21).

### Requirements that are new

| Id | Requirement | Test |
|---|---|---|
| OBX-22 | `connect_existing` refuses to start unless both tables, their indexes, and their triggers are present, and `connect` creates all of them. There is no option to run without the triggers. | `a_missing_trigger_stops_the_relay`, `connect_makes_the_triggers` |
| OBX-23 | Events and attempts are only ever inserted. `UPDATE`, `DELETE` and `TRUNCATE` on either table are refused by the database, whoever sends them, apart from the one erasure function (OBX-30). | `the_tables_refuse_every_change` |
| OBX-24 | A row's state is read from its latest attempt, and never stored on the event. `stats` counts `pending`, `held`, `delivered`, `refused` and `failed`, and the age of the oldest of each that is still owed. | `state_is_read_from_the_attempts`, `stats_count_every_outcome` |
| OBX-25 | `Refuse(reason)` is final. The row is never claimed again, `release` refuses it, and it frees its key as `delivered` does. | `a_refusal_is_final`, `a_refusal_cannot_be_released`, `a_refusal_frees_its_key` |
| OBX-26 | `Hold` writes no attempt, charges nothing, does not grow the backoff, and leaves the row to be decided again at the next round, for as long as the hold lasts. A held row holds back its own key, and no other. | `a_hold_charges_nothing`, `a_hold_lasts_as_long_as_it_is_given`, `a_hold_holds_back_only_its_key` |
| OBX-27 | `Outbox::run` takes a decision closure, run before the publisher in the claim's transaction, answering `Deliver`, `Refuse(reason)` or `Hold`. It is required. A service with no rule passes `decide::deliver_all`, which says so in its own code. A refused event never reaches the publisher. | `a_refused_event_never_reaches_the_publisher`, `run_needs_a_decision` |
| OBX-28 | `Outbox::run` takes a record closure, run after each outcome is written, in that outcome's transaction, before it commits. It is required. A service with nothing to audit passes `record::nothing`. If it fails, the outcome rolls back and the row stays claimed, to be recovered. | `an_outcome_and_its_audit_commit_together`, `a_failed_record_rolls_back_the_outcome` |
| OBX-29 | Every attempt names its relay and its claim. A relay given up on still cannot write a later outcome over the relay that took over (OBX-6 holds). Its late success is kept as an attempt of its own, so a duplicate delivery is on record, not merged into another. | `each_attempt_names_its_relay_and_claim`, `a_late_success_is_kept_as_its_own_attempt` |
| OBX-30 | Nothing is swept, and there is no `retention_days`. A payload that must go is erased with `erase(tx, event_id, by, reason)`. It appends an `erased` attempt, and the one `SECURITY DEFINER` function the triggers allow sets that event's payload to `NULL`, changing nothing else. | `nothing_is_swept`, `an_erasure_blanks_only_the_payload_and_is_kept`, `nothing_else_can_blank_a_payload` |
| OBX-31 | A fingerprint, SHA-256 over topic, kind, key and payload, is stored at write and checked before the decision. A row that no longer matches is `failed: tampered`, never published, and is counted in `stats`. | `a_changed_row_is_never_published` |
| OBX-32 | A reason is at most 200 characters with no line breaks, because it is a code and not content. A longer one is cut to `reason_too_long`, and logged without its text. | `a_reason_is_a_short_code` |
| OBX-33 | A payload over `max_payload_bytes`, 64 KiB by default, is refused at write, so the caller's transaction fails rather than storing a large copy of personal data. | `a_large_payload_fails_the_write` |

### The tables

```sql
CREATE TABLE IF NOT EXISTS axa_outbox_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    topic TEXT NOT NULL,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    payload JSONB,              -- NULL only after an erasure
    fingerprint TEXT NOT NULL,  -- SHA-256 of topic, kind, key and payload, at write
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS axa_outbox_attempts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id UUID NOT NULL REFERENCES axa_outbox_events (event_id),
    outcome TEXT NOT NULL CHECK (outcome IN
        ('claimed', 'delivered', 'refused', 'failed', 'released', 'erased')),
    relay UUID NOT NULL,
    claim UUID NOT NULL,
    reason TEXT CHECK (length(reason) <= 200),
    by TEXT,                    -- who released or erased; NULL for a relay
    at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

The setup statements also make triggers refusing `UPDATE`, `DELETE` and `TRUNCATE` on
both tables, and one `SECURITY DEFINER` function, `axa_outbox_erase`. It is the only
path that may set `payload` to `NULL`, and only alongside an `erased` attempt. Indexes
find what is owed without reading delivered rows (OBX-16): the latest attempt per
event, and events with none. A claim is an attempt, and a heartbeat is a newer claim,
so recovery (OBX-5) works from the attempts.

The attempts table grows with every claim and heartbeat. A heartbeat every
`heartbeat_interval` across a long batch is the largest source. If that proves too
much, the proposal's fallback is a separate, mutable `axa_outbox_leases` table holding
only the live lease: state, under P-7, and so allowed to change. The record of
outcomes stays insert-only either way.

### The API, roughly

```rust,ignore
use axa::outbox::{Decision, Failure, Outbox, OutboxOptions};

let outbox = Outbox::connect_existing(uri, OutboxOptions::default()).await?;

tokio::spawn(outbox.run(
    // Decide, in the claim's transaction: the service's own policy, consent, holds.
    |event, tx| async move { decide(tx, &event).await },        // Deliver | Refuse(r) | Hold
    // Publish, as in 0.1.0.
    |event| async move { publish(&event).await.map_err(Failure::unavailable) },
    // Record, in the outcome's transaction: the service's audit row.
    |event, outcome, tx| async move { audit(tx, &event, &outcome).await },
    stopped,
));
```

A service with its own loop has `claim`, `delivered(tx, …)`, `refused(tx, …)`,
`failed(tx, …)`, `release(tx, …, by, reason)` and `erase(tx, …, by, reason)`. Each one
takes the transaction, and there is no form that takes a pool.

## Upgrading from 0.1.0

- `SETUP_STATEMENTS` makes the two new tables, their triggers and the erasure
  function. `axa_outbox` is not touched, so an upgrade has nothing to roll back.
- `IMPORT_STATEMENTS` copies each 0.1.0 row into `axa_outbox_events`, with a
  fingerprint, and appends one attempt for its state, `imported: pending`,
  `imported: sent` or `imported: failed`, keeping its attempt count in the reason. A
  service runs them in a migration with its relays stopped, then drops `axa_outbox` in
  a later migration of its own. Rows 0.1.0 already swept cannot be recovered, and the
  help page says so.
- `Outbox::run` needs a decision and a record closure. A service that had neither
  passes `decide::deliver_all` and `record::nothing`, which is one line and is visible
  in review.
- `retention_days` and `retry_failed` are gone. `release` and `erase` take who and why.
- No new dependency: SHA-256 is already in the workspace.

## Also in 0.2.0

- **Order per topic.** Strict per-key order (OBX-8) stays as the rule, and a topic can
  pass `Order::None` where one stuck key must not hold back the rest. Ordering is not
  security, so a choice here does not conflict with P-10.
- **An inbox for consumers.** At least once (OBX-3) means every consumer dedupes. A small
  insert-only `axa_inbox` keyed by `event_id`, with `seen(tx, event_id) -> bool` in the
  consumer's transaction, makes the right thing the easy one.
- **Documentation that prefers an id to a snapshot.** For a health record, a snapshot
  is a second copy of patient data, and OBX-33's limit is a backstop, not the advice.

## Out of scope

- Evaluating policy or consent. That stays in the service, and the decision closure is
  how it gets there, consistent with section 2.
- A broker client. OBX-21 holds.
- Encryption of payloads at rest. That belongs in the database or the service's own
  key management, not here.

## Open questions

1. Should a `Hold` that lasts beyond a limit the service sets surface as an alert?
   Proposed: no limit and no change of state. `stats` gives the count and age of the
   oldest held row, so a forgotten hold is visible and nothing acts on it unasked.
2. P-2 as amended admits code with one consumer. Is the safety-case test strict enough,
   or should it also need a written hazard or control that names the requirement?
3. Heartbeats as attempts, or a mutable lease table (see "The tables")? The first is
   simpler to reason about; the second keeps the record small.
4. P-10 makes the `axa-idempotency`, `axa-worker` and `axa-offset-pagination` changes
   above due as well. Do they ship in the same breaking release as this one, or each in
   its own?
