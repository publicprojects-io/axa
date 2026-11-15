# Upgrading to Axa 0.15

This covers a service on 0.14. Most services upgrade by changing the version. A service
on `connect_existing` runs the job queue's setup again first, and a service that calls
MySQL's `enqueue_on` passes it a connection.

## What will not compile

| Change | What to do |
|---|---|
| `axa_mysql_job_queue::enqueue_on` takes a `&mut MySqlConnection` rather than any executor | Pass `&mut transaction`, or `&mut *connection` for a pooled connection. `Queue::enqueue_job` is unchanged |
| `SETUP_STATEMENTS` in each job queue has more statements | Nothing, unless you match on the array's length |

## What changes with no compile error

**The job queue has a `unique_key` column and the `axa_queue_unique` index**, and
`axa_schedule` has a `cron` column. `Queue::connect` adds them at boot. A service on
`Queue::connect_existing` runs `SETUP_STATEMENTS` again in a migration before it deploys
0.15, as `check` refuses tables without them. Each statement is safe to repeat, on
Postgres, MySQL, and SQLite alike. On MySQL, setup also adds a virtual `unique_slot`
column, which holds the unique index, as MySQL has no partial index.

**`retry` leaves a failed job alone while another of its key is queued or running.**
This touches only jobs queued with `NewJob::unique`, which is new, so no job queued in
0.14 has a key.

## What is new

- `NewJob::unique(key)` queues a job once while another of the same name, tenant, and
  key is queued or running, and answers that job's id. See
  [One job at a time per key](../background-jobs/index.md#one-job-at-a-time-per-key).
- `Schedule::cron` and `Queue::cron` queue a job at each time a cron pattern names, in
  UTC. See [Cron schedules](../background-jobs/index.md#cron-schedules).
- The `mail` feature, `axa::mail`, from `axa-mail`: SMTP with TLS, a directory of
  `.eml` files, or an outbox in memory for tests.
- The `storage` feature, `axa::storage`, from `axa-storage-with-apache-arrow-and-aws`:
  objects on S3, local disk, or in memory, with presigned URLs on S3.

The published `axa` still leaves out the `sqlite-*` features, as in 0.13 and 0.14.
