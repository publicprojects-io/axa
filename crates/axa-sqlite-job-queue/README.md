# axa-sqlite-job-queue

A SQLite job queue for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family: a pool of job loops with per-job-type limits, delayed jobs, priorities, a job's status by id, and recurring schedules that enqueue each due time once. It is the SQLite twin of `axa-postgres-job-queue`, with the same API, for a service on one machine.

Jobs live in one `axa_queue` table. A loop claims a job in one `UPDATE ... RETURNING`, and SQLite runs one writer at a time, so two loops, or two processes on the same file, never claim the same job. Schedules live in `axa_schedule`, and each due time becomes an ordinary job. The database runs in WAL mode, with a five-second wait for the write lock.

A job row is state, and retention deletes it. A person's act on a job is a record. `cancel` and `retry` take the caller's transaction, a `by`, and a `reason`, and append the act to `axa_job_acts` with the change, so the service's own audit row commits with both. That table is only ever inserted into, and its triggers refuse `UPDATE` and `DELETE`. `Queue::acts` reads a job's acts, which outlive its row.

Depend on this crate and register closures; it is not yet a feature of `axa`. See the crate documentation for usage.

## Security advisory

This crate builds the `libsqlite3-sys` that sqlx 0.9.0 allows, which is below 0.38 and has
[RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html): a crafted
database and `-wal` file pair can make queries return uninitialized memory. It is local,
and needs files a less trusted party wrote. Point the queue only at a database file your
service creates and owns. Any build on sqlx's SQLite driver has the same library, and one
crate may link `sqlite3`, so this crate cannot take the fix in 0.39 until sqlx raises its
cap ([sqlx PR #4502](https://github.com/transact-rs/sqlx/pull/4502), merged and not yet
released). The next release after that will build the fixed library.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
