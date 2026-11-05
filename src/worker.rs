//! Background jobs.

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{AppContext, Error, Result};

/// How [`BackgroundWorker::perform_later`] runs a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerMode {
    /// Enqueue on the Postgres queue for a worker process.
    BackgroundQueue,
    /// Run inside the request. Used by tests.
    ForegroundBlocking,
    /// Spawn on the current runtime.
    BackgroundAsync,
}

/// A job type. `A` is its serializable argument.
#[async_trait]
pub trait BackgroundWorker<A>: Send + Sync
where
    A: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Build the worker.
    fn build(ctx: &AppContext) -> Self;

    /// Name recorded on queued jobs.
    #[must_use]
    fn class_name() -> String
    where
        Self: Sized,
    {
        let name = std::any::type_name::<Self>();
        name.rsplit("::").next().unwrap_or(name).to_string()
    }

    /// Run or enqueue a job, per [`WorkerMode`], and return its id.
    ///
    /// # Errors
    ///
    /// Fails when queueing fails, no queue is configured, or a foreground
    /// job fails.
    async fn perform_later(ctx: &AppContext, args: A) -> Result<String>
    where
        Self: Sized + 'static,
    {
        match ctx.config.workers {
            WorkerMode::BackgroundQueue => enqueue::<A>(ctx, &Self::class_name(), &args).await,
            WorkerMode::ForegroundBlocking => {
                Self::build(ctx).perform(args).await?;
                Ok(Uuid::new_v4().to_string())
            }
            WorkerMode::BackgroundAsync => {
                let worker = Self::build(ctx);
                tokio::spawn(async move {
                    if let Err(error) = worker.perform(args).await {
                        tracing::error!(%error, "background job failed");
                    }
                });
                Ok(Uuid::new_v4().to_string())
            }
        }
    }

    /// The job's work.
    async fn perform(&self, args: A) -> Result<()>;
}

#[cfg(feature = "queue")]
async fn enqueue<A: Serialize + Sync>(ctx: &AppContext, name: &str, args: &A) -> Result<String> {
    let queue = ctx
        .queue_provider
        .as_ref()
        .ok_or_else(|| Error::Worker("no job queue configured".to_string()))?;
    queue.enqueue(name, serde_json::to_value(args)?).await
}

#[cfg(not(feature = "queue"))]
async fn enqueue<A: Serialize + Sync>(_ctx: &AppContext, _name: &str, _args: &A) -> Result<String> {
    Err(Error::Worker("no job queue configured".to_string()))
}
