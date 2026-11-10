# axa-mysql-job-queue

A MySQL job queue for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family: a pool of job loops with per-job-type limits, delayed jobs, priorities, a job's status by id, and recurring schedules that enqueue each due time once across replicas. It is the MySQL twin of `axa-postgres-job-queue`, with the same API.

Jobs live in one `axa_queue` table and are claimed with `FOR UPDATE SKIP LOCKED`, so no broker is needed. Schedules live in `axa_schedule`, and each due time becomes an ordinary job, so retries, status, limits, and recovery are the queue's.

A job row is state, and retention deletes it. A person's act on a job is a record. `cancel` and `retry` take the caller's transaction, a `by`, and a `reason`, and append the act to `axa_job_acts` with the change, so the service's own audit row commits with both. That table is only ever inserted into, and its triggers refuse `UPDATE` and `DELETE`. MySQL has no `TRUNCATE` trigger, so give the service's role no `DROP` privilege on it. `Queue::acts` reads a job's acts, which outlive its row.

It needs MySQL 8.4 or later. MariaDB is not supported. Depend on this crate and register closures; it is not yet a feature of `axa`. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
