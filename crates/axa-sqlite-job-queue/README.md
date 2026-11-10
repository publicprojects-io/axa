# axa-sqlite-job-queue

A SQLite job queue for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family: a pool of job loops with per-job-type limits, delayed jobs, priorities, a job's status by id, and recurring schedules that enqueue each due time once. It is the SQLite twin of `axa-postgres-job-queue`, with the same API, for a service on one machine.

Jobs live in one `axa_queue` table. A loop claims a job in one `UPDATE ... RETURNING`, and SQLite runs one writer at a time, so two loops, or two processes on the same file, never claim the same job. Schedules live in `axa_schedule`, and each due time becomes an ordinary job. The database runs in WAL mode, with a five-second wait for the write lock.

A job row is state, and retention deletes it. A person's act on a job is a record. `cancel` and `retry` take the caller's transaction, a `by`, and a `reason`, and append the act to `axa_job_acts` with the change, so the service's own audit row commits with both. That table is only ever inserted into, and its triggers refuse `UPDATE` and `DELETE`. `Queue::acts` reads a job's acts, which outlive its row.

Depend on this crate and register closures; it is not yet a feature of `axa`. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
