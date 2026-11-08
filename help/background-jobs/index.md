# Background jobs and schedules

`axa-worker` is the Postgres job queue behind the `queue` feature. It runs jobs in a
pool of loops, delays and orders them, says where each job is, and queues jobs on a
schedule. This guide says how a service uses each part, and what it must decide.

## Registering and queuing

A job type is a `BackgroundWorker`. Register each one on the queue, and queue a job
with `perform_later`, which follows `WORKER_MODE`:

```rust
let mut queue = Queue::connect(&uri, QueueOptions::default()).await?;
queue.register(ReportWorker::build(&ctx));
ctx.queue_provider = Some(Arc::new(queue));

ReportWorker::perform_later(&ctx, Args { id }).await?;
ReportWorker::perform_in(&ctx, Args { id }, Duration::from_mins(10)).await?;
```

To queue a job with the record it is about, use the caller's transaction:

```rust
let mut transaction = pool.begin().await?;
// ... write the record ...
queue::enqueue_on::<ReportWorker, _, _>(&mut *transaction, &Args { id }).await?;
transaction.commit().await?;
```

`queue::job::<W, _>(&args)?` builds a job you can delay with `run_in` or `run_at`, put
ahead of others with `priority`, or give a `timeout`, and then queue with
`queue::enqueue_job_on` or `Queue::enqueue_job`. `Queue::enqueue_all` queues a batch in
one transaction, so all or none.

## How many at once

`QueueOptions::workers` is how many job loops a process runs, so the most jobs it runs
at once. It is 2 by default. To keep one job type below that, so that a slow report
cannot take every loop:

```rust
queue.limit::<_, ReportWorker>(1);
```

A limit counts jobs in this process. Three worker processes with a limit of one may run
three reports at once. Work that must never overlap anywhere needs a lock of its own,
such as a unique constraint or an advisory lock.

## Time limits

A job has no time limit unless you give it one. Set one on the worker, and every run
of its jobs past it is stopped and counted as failed, with `timed out after` and the
limit as its error:

```rust
fn timeout() -> Option<Duration> {
    Some(Duration::from_mins(5))
}
```

A timed-out job follows the same rule as any failure: it is tried again only if it has
attempts left. There is no default limit, and none is taken from a schedule's interval.

## Where a job is

`perform_later` and the enqueue functions return the job's id. Keep it to ask later:

| Call | What it does |
|---|---|
| `queue.job(&id)` | The job's state, attempts, priority, time limit, times, and last error. Never its arguments |
| `queue::cancel(&mut tx, &id, by, reason)` | Cancels a job that is still queued, as an act of `by`. A running job runs on |
| `queue::retry(&mut tx, &id, by, reason)` | Queues a failed job, or releases one waiting for review, for one more try, as an act of `by` |
| `queue.acts(&id)` | Every act a person did to the job, oldest first, with who, why, and when |
| `queue.counts()` | How many are ready, waiting, running, finished, failed, cancelled, or waiting for review, and how long the oldest ready job has waited |

These are library calls. A service that shows them to users exposes them through its
own routes, behind its own authorisation. A job's error text is whatever its error
said, so it is for operators, and never goes to a client as it is.

Export `counts` as metrics. A rising `oldest_ready_seconds` means jobs are not being
run, and anything in `needs_review` needs a person.

## A person's acts

A job row is state. It changes as the job runs, and retention deletes it after
`retention_days`. What a person does to a job is a record, so it is kept apart, in
`axa_job_acts`.

`cancel` and `retry` take the caller's transaction, who did it, and why. Each changes
the job and appends the act in one statement. Write your own audit row on the same
transaction, and all three commit together or not at all:

```rust
let mut transaction = pool.begin().await?;
if queue::cancel(&mut transaction, &id, &user.id, "customer_asked").await? {
    // ... write the service's own audit row ...
}
transaction.commit().await?;
```

There is no form that takes a pool. An act that does not apply, such as cancelling a
job already running, answers `false`, changes nothing, and keeps no act. A `retry` of a
failed job is kept as `retried`, and of one in review as `released`.

`by` and `reason` are short codes, not free text: 1 to 200 characters, with no line
breaks. Anything else is refused with `Error::BadRequest`, before anything is written.
Name the person by an id your service keeps, and give a reason such as `duplicate`,
not a sentence about the customer.

`axa_job_acts` is only ever inserted into. Its triggers refuse `UPDATE`, `DELETE`, and
`TRUNCATE` from any role, even with `session_replication_role` set to `replica`. An act
names its job by id, with no foreign key, and keeps the job's name and attempts, so it
outlives the job's row. `queue.acts(&id)` reads them for your own review screens.

## A run that lost its claim

A running job reports in every `heartbeat_interval`. Another worker takes a job not
heard from for `stale_minutes` for lost with its process, and queues it again. When the
first run is in fact still going, after a long database outage for example, it finds
its claim gone. That run is **stopped**, and the job is marked `needs_review` with the
reason, because it may have done part of its work, or may now be running twice.

A job in `needs_review` is never tried again by itself: not by stale recovery, not by
retention, and not by its attempts. A person checks what it did, and calls
`queue::retry(&mut tx, &id, by, reason)` to let it run again, which is kept as a
`released` act. A run that finishes but finds its claim gone
when it writes its result is marked the same way.

## Schedules

A schedule queues an ordinary job every interval. It is built in code, beside the
worker registrations:

```rust
queue.register(SweepWorker::build(&ctx));
queue.every::<_, SweepWorker>("idempotency-sweep", Duration::from_hours(1), &())?;
```

Two jobs that every service with these crates should schedule:

- **`axa-idempotency`'s sweep.** Expired answers stay on disk until it runs. A
  `SweepWorker` whose `perform` calls `keys.sweep().await` every hour does it.
- **Retention of a service's own tables**, the way `axa-outbox` sweeps sent rows after
  `retention_days`: a worker that runs one `DELETE ... WHERE created_at < now() - ...`.

Each due time becomes one job, so its retries, status, time limit, and recovery are the
queue's. Across any number of worker processes, each due time is queued once: the
schedule's row is locked while its job is queued, in the same transaction.

- **No drift.** The next time is counted from when the last was due.
- **Missed times run once.** After every process was down for a day, one job is queued,
  and the next time stays a whole number of intervals from the first.
- **No stacking.** A due time is skipped while the schedule's last job is still queued
  or running. A last job that failed, was cancelled, or waits for review does not hold
  the next one back.
- **The first time** is as soon as the schedule's row is made, unless you give one:

```rust
let first = /* the next 02:00 UTC */;
queue.schedule(Schedule::every("nightly", Duration::from_hours(24), job).first_at(first));
```

The first time is used only when the row is made, so a schedule every 24 hours first
due at 02:00 runs at 02:00 each day, on the database's clock in UTC. A more general
"daily at 02:00" form, with time zones, is later work.

The worker process ticks schedules on every poll, `poll_interval`, 1 second by default,
so a due job is queued within about that. A process with no job loops can call
`queue.tick_schedules()` itself.

A schedule name keys its row, so keep it stable. A name that is empty, over 200
characters, or used twice, or an interval under a second, panics at boot, as Axum does
for a bad route.

### Removing a schedule

A schedule taken out of code is **archived**, never deleted, so its history can be
audited. Each process that has a schedule in code marks its row as seen, at most once
a minute. Housekeeping archives a row no process has seen for `stale_minutes`, so an
old replica that still has the schedule during a rolling deploy keeps it alive until
it stops, and the new one does not fight it. Archiving the same row again does
nothing.

An archived schedule queues nothing. A job it already queued runs as usual.
`queue.schedules()` lists every row, archived or not, with its interval, next due
time, when it was last seen, and its last job's state and error.

When a schedule of that name returns to code, the same row is restored, not a new one
made. It keeps its place: `first_at` does not apply again, and the times missed while
it was archived come to one job, or none if its last job has not finished.

## Upgrading

Run `queue::SETUP_STATEMENTS` again in a migration. They add the `priority` and
`timeout_ms` columns, the `axa_schedule` table, and the `axa_job_acts` table and its
triggers, and `Queue::connect_existing` refuses to start without them, or with a
trigger turned off. `Queue::connect` adds them by itself. They need Postgres 14 or
later.

Stop old workers before new ones take over, or drain the queue first. An old worker
does not know `needs_review` or `cancelled`, but it never claims them either, as it
claims only `queued` rows.
