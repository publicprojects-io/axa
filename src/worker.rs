//! Background jobs.

use std::time::Duration;

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
        Self::perform_in(ctx, args, Duration::ZERO).await
    }

    /// Run or enqueue a job no sooner than `delay` from now, per [`WorkerMode`], and
    /// return its id. On the queue the delay is kept on the job's row.
    /// `BackgroundAsync` sleeps for it in this process, so a restart loses that job,
    /// and `ForegroundBlocking` runs the job straight away, as tests want.
    ///
    /// # Errors
    ///
    /// Fails when queueing fails, no queue is configured, or a foreground
    /// job fails.
    async fn perform_in(ctx: &AppContext, args: A, delay: Duration) -> Result<String>
    where
        Self: Sized + 'static,
    {
        match ctx.config.workers {
            WorkerMode::BackgroundQueue => {
                #[cfg(feature = "queue")]
                {
                    let job = crate::queue::job::<Self, A>(&args)?.run_in(delay);
                    enqueue(ctx, &job).await
                }
                #[cfg(not(feature = "queue"))]
                {
                    let _ = (args, delay);
                    no_queue()
                }
            }
            WorkerMode::ForegroundBlocking => {
                Self::build(ctx).perform(args).await?;
                Ok(Uuid::new_v4().to_string())
            }
            WorkerMode::BackgroundAsync => {
                let worker = Self::build(ctx);
                tokio::spawn(async move {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    if let Err(error) = worker.perform(args).await {
                        tracing::error!(%error, "background job failed");
                    }
                });
                Ok(Uuid::new_v4().to_string())
            }
        }
    }

    /// Tries a job of this worker gets with the queue, including the first.
    /// `None` uses `QueueOptions::max_attempts`. Set it only for work that is
    /// safe to repeat.
    #[must_use]
    fn max_attempts() -> Option<i32>
    where
        Self: Sized,
    {
        None
    }

    /// How long a run of this worker's job may take before it is stopped and
    /// counted as failed. `None`, the default, sets no limit: a job has one only
    /// when its worker says so.
    #[must_use]
    fn timeout() -> Option<Duration>
    where
        Self: Sized,
    {
        None
    }

    /// The job's work.
    async fn perform(&self, args: A) -> Result<()>;
}

/// Add a job to the context's queue.
#[cfg(feature = "queue")]
async fn enqueue(ctx: &AppContext, job: &crate::queue::NewJob) -> Result<String> {
    let queue = ctx
        .queue_provider
        .as_ref()
        .ok_or_else(|| Error::Worker("no job queue configured".to_string()))?;
    queue.enqueue_job(job).await
}

/// Without the `queue` feature there is nowhere to put a job.
#[cfg(not(feature = "queue"))]
fn no_queue() -> Result<String> {
    Err(Error::Worker("no job queue configured".to_string()))
}

#[cfg(all(
    test,
    not(any(axa_db = "seaorm", axa_db = "toasty")),
    not(feature = "queue")
))]
mod tests {
    use super::*;
    use crate::{
        Environment,
        config::{Config, Defaults},
    };
    use serde::Deserialize;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DONE: AtomicUsize = AtomicUsize::new(0);

    #[derive(Serialize, Deserialize)]
    struct Args {
        amount: usize,
        fail: bool,
    }

    struct AddWorker;

    #[async_trait]
    impl BackgroundWorker<Args> for AddWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        async fn perform(&self, args: Args) -> Result<()> {
            if args.fail {
                return Err(Error::Worker("told to fail".to_string()));
            }
            DONE.fetch_add(args.amount, Ordering::SeqCst);
            Ok(())
        }
    }

    fn context(mode: &str) -> AppContext {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |name| {
            (name == "WORKER_MODE").then(|| mode.to_string())
        })
        .unwrap();
        AppContext::for_test(Environment::Test, config)
    }

    #[test]
    fn class_name_is_the_bare_type_name() {
        assert_eq!(AddWorker::class_name(), "AddWorker");
    }

    #[tokio::test]
    async fn foreground_jobs_run_before_returning_and_report_failure() {
        let ctx = context("ForegroundBlocking");
        let before = DONE.load(Ordering::SeqCst);
        AddWorker::perform_later(
            &ctx,
            Args {
                amount: 3,
                fail: false,
            },
        )
        .await
        .unwrap();
        assert!(DONE.load(Ordering::SeqCst) >= before + 3);
        assert!(
            AddWorker::perform_later(
                &ctx,
                Args {
                    amount: 0,
                    fail: true
                }
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn queued_jobs_fail_without_a_queue() {
        let ctx = context("BackgroundQueue");
        let error = AddWorker::perform_later(
            &ctx,
            Args {
                amount: 1,
                fail: false,
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("no job queue"));
    }
}
