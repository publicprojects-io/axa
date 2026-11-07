# axa-worker

A Postgres job queue for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family: a pool of job loops with per-job-type limits, delayed jobs, priorities, a job's status by id, and recurring schedules that enqueue each due time once across replicas.

Jobs live in one `axa_queue` table and are claimed with `FOR UPDATE SKIP LOCKED`, so no broker is needed. Schedules live in `axa_schedule`, and each due time becomes an ordinary job, so retries, status, limits, and recovery are the queue's.

Enable it through the `queue` feature of `axa`, which adds the `BackgroundWorker` trait over it, or depend on this crate alone and register closures. See the crate documentation for usage, and the [guide](https://github.com/publicprojects-io/axa/blob/main/help/background-jobs/index.md) for what a service changes.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
