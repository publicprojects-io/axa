//! A Postgres job queue using `FOR UPDATE SKIP LOCKED`.

use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use serde::{Serialize, de::DeserializeOwned};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{Error, Result, worker::BackgroundWorker};

type JobFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type Handler = Arc<dyn Fn(serde_json::Value) -> JobFuture + Send + Sync>;

const SETUP: &str = "CREATE TABLE IF NOT EXISTS axa_queue (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    args JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    error TEXT,
    run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
)";
const SETUP_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS axa_queue_ready ON axa_queue (run_at) WHERE status = 'queued'";
const ENQUEUE: &str = "INSERT INTO axa_queue (id, name, args) VALUES ($1, $2, $3)";
const CLAIM: &str = "UPDATE axa_queue
    SET status = 'running', started_at = now(), updated_at = now()
    WHERE id = (
        SELECT id FROM axa_queue
        WHERE status = 'queued' AND run_at <= now() AND name = ANY($1)
        ORDER BY run_at
        FOR UPDATE SKIP LOCKED
        LIMIT 1
    )
    RETURNING id, name, args";
const FINISH: &str =
    "UPDATE axa_queue SET status = $2, error = $3, updated_at = now() WHERE id = $1";
const REQUEUE_STALE: &str = "UPDATE axa_queue
    SET status = 'queued', started_at = NULL, updated_at = now()
    WHERE status = 'running' AND started_at < now() - make_interval(mins => $1)";

/// Queue tuning.
#[derive(Debug, Clone)]
pub struct QueueOptions {
    /// Concurrent job loops.
    pub workers: usize,
    /// Sleep between polls of an empty queue.
    pub poll_interval: Duration,
    /// Jobs running longer than this many minutes are assumed lost with
    /// their worker and queued again.
    pub stale_minutes: i32,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            workers: 2,
            poll_interval: Duration::from_secs(1),
            stale_minutes: 60,
        }
    }
}

/// A Postgres-backed job queue.
pub struct Queue {
    pool: PgPool,
    handlers: HashMap<String, Handler>,
    options: QueueOptions,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("handlers", &self.handlers.len())
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Queue {
    /// Connect and create the queue table if needed.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached or the table not created.
    pub async fn connect(uri: &str, options: QueueOptions) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(
                u32::try_from(options.workers)
                    .unwrap_or(2)
                    .saturating_add(1),
            )
            .connect(uri)
            .await?;
        sqlx::query(SETUP).execute(&pool).await?;
        sqlx::query(SETUP_INDEX).execute(&pool).await?;
        Ok(Self {
            pool,
            handlers: HashMap::new(),
            options,
        })
    }

    /// Register a worker to handle its jobs.
    pub fn register<A, W>(&mut self, worker: W)
    where
        A: Serialize + DeserializeOwned + Send + Sync + 'static,
        W: BackgroundWorker<A> + 'static,
    {
        let worker = Arc::new(worker);
        let handler: Handler = Arc::new(move |args| {
            let worker = worker.clone();
            Box::pin(async move { worker.perform(serde_json::from_value(args)?).await })
        });
        self.handlers.insert(W::class_name(), handler);
    }

    /// Add a job and return its id.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn enqueue(&self, name: &str, args: serde_json::Value) -> Result<String> {
        let id = Uuid::new_v4();
        sqlx::query(ENQUEUE)
            .bind(id)
            .bind(name)
            .bind(args)
            .execute(&self.pool)
            .await?;
        Ok(id.to_string())
    }

    /// Check the database connection.
    ///
    /// # Errors
    ///
    /// Fails when the database does not answer.
    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Queue again any job abandoned by a worker that died, returning how
    /// many were requeued.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn requeue_stale(&self) -> Result<u64> {
        let result = sqlx::query(REQUEUE_STALE)
            .bind(self.options.stale_minutes)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Claim and run one job, returning whether there was one. A job's own
    /// failure is recorded on its row, not returned.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn run_one(&self) -> Result<bool> {
        let names: Vec<String> = self.handlers.keys().cloned().collect();
        let Some(row) = sqlx::query(CLAIM)
            .bind(&names)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(false);
        };
        let id: Uuid = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let args: serde_json::Value = row.try_get("args")?;
        let outcome = match self.handlers.get(&name) {
            Some(handler) => handler(args).await,
            None => Err(Error::Worker(format!("no handler for job {name}"))),
        };
        let (status, error) = match outcome {
            Ok(()) => ("completed", None),
            Err(error) => {
                tracing::error!(job = %name, %id, %error, "job failed");
                ("failed", Some(error.to_string()))
            }
        };
        sqlx::query(FINISH)
            .bind(id)
            .bind(status)
            .bind(error)
            .execute(&self.pool)
            .await?;
        Ok(true)
    }

    /// Run job loops until `shutdown` changes to `true`. Jobs in progress
    /// finish first.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) {
        let mut loops = Vec::new();
        for _ in 0..self.options.workers.max(1) {
            let queue = self.clone();
            let mut shutdown = shutdown.clone();
            loops.push(tokio::spawn(async move {
                while !*shutdown.borrow() {
                    let worked = queue.run_one().await.unwrap_or_else(|error| {
                        tracing::error!(%error, "job queue error");
                        false
                    });
                    if !worked {
                        tokio::select! {
                            () = tokio::time::sleep(queue.options.poll_interval) => {},
                            _ = shutdown.changed() => {},
                        }
                    }
                }
            }));
        }
        for job_loop in loops {
            let _ = job_loop.await;
        }
    }
}
