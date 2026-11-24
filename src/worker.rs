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

    /// Run or enqueue a job for `tenant`, per [`WorkerMode`], and return its id. The
    /// job runs in [`BackgroundWorker::perform_for`], which this worker must
    /// implement to take a tenant: a job runs outside any request, so its tenant is
    /// lost unless the job carries it. Take `tenant` from the auth crate's `Tenant`,
    /// as `tenant.as_str()`, never from what a caller sent.
    ///
    /// # Errors
    ///
    /// Fails for a tenant that is not a tenant id, when queueing fails, no queue is
    /// configured, or a foreground job fails.
    async fn perform_later_for(ctx: &AppContext, tenant: &str, args: A) -> Result<String>
    where
        Self: Sized + 'static,
    {
        if !is_tenant_id(tenant) {
            return Err(Error::BadRequest(
                "a job's tenant must be a tenant id".to_string(),
            ));
        }
        let tenant = tenant.to_string();
        match ctx.config.workers {
            WorkerMode::BackgroundQueue => {
                #[cfg(feature = "queue")]
                {
                    let job = crate::queue::job::<Self, A>(&args)?.tenant(tenant);
                    enqueue(ctx, &job).await
                }
                #[cfg(not(feature = "queue"))]
                {
                    let _ = (args, tenant);
                    no_queue()
                }
            }
            WorkerMode::ForegroundBlocking => {
                Self::build(ctx).perform_for(Some(tenant), args).await?;
                Ok(Uuid::new_v4().to_string())
            }
            WorkerMode::BackgroundAsync => {
                let worker = Self::build(ctx);
                tokio::spawn(async move {
                    if let Err(error) = worker.perform_for(Some(tenant), args).await {
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

    /// The job's work for `tenant`, the tenant it was queued for, or `None`. A worker
    /// whose jobs run for a tenant implements this, and opens its own tenant
    /// transaction with it. By default a job with no tenant goes to
    /// [`BackgroundWorker::perform`], and a job with one is refused, as `perform`
    /// would run it with no tenant.
    ///
    /// # Errors
    ///
    /// By default, for a job with a tenant, and otherwise as `perform`.
    async fn perform_for(&self, tenant: Option<String>, args: A) -> Result<()> {
        match tenant {
            None => self.perform(args).await,
            Some(_) => Err(Error::Worker(
                "the job has a tenant, and its worker takes none: implement perform_for"
                    .to_string(),
            )),
        }
    }
}

/// Whether `id` is a tenant id, as the auth crates' `Tenant` and the job queue hold
/// it: 1 to 63 lower-case letters, digits, and hyphens, not starting or ending with a
/// hyphen.
fn is_tenant_id(id: &str) -> bool {
    (1..=63).contains(&id.len())
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
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
    not(any(
        axa_db = "postgres_seaorm",
        axa_db = "mysql_seaorm",
        axa_db = "sqlite_seaorm",
        axa_db = "postgres_toasty",
        axa_db = "mysql_toasty"
    ))
))]
#[expect(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
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
        AppContext::with_test_db(Environment::Test, config)
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

    struct TenantWorker;

    static TENANTS: std::sync::Mutex<Vec<Option<String>>> = std::sync::Mutex::new(Vec::new());

    #[async_trait]
    impl BackgroundWorker<()> for TenantWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        async fn perform(&self, (): ()) -> Result<()> {
            Err(Error::Worker("needs a tenant".to_string()))
        }

        async fn perform_for(&self, tenant: Option<String>, (): ()) -> Result<()> {
            TENANTS.lock().unwrap().push(tenant);
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_job_for_a_tenant_reaches_perform_for_and_a_worker_that_takes_none_refuses_it() {
        let ctx = context("ForegroundBlocking");
        TenantWorker::perform_later_for(&ctx, "acme", ())
            .await
            .unwrap();
        assert_eq!(*TENANTS.lock().unwrap(), vec![Some("acme".to_string())]);
        let args = Args {
            amount: 1,
            fail: false,
        };
        let error = AddWorker::perform_later_for(&ctx, "acme", args)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("perform_for"), "{error}");
        for bad in ["", "Acme", "acme-", &"a".repeat(64)] {
            let error = TenantWorker::perform_later_for(&ctx, bad, ())
                .await
                .unwrap_err();
            assert!(matches!(error, Error::BadRequest(_)), "{bad:?}");
        }
        assert_eq!(TENANTS.lock().unwrap().len(), 1);
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
        let error = TenantWorker::perform_later_for(&ctx, "acme", ())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no job queue"), "{error}");
    }

    static LATER: AtomicUsize = AtomicUsize::new(0);
    static LATER_TENANTS: std::sync::Mutex<Vec<Option<String>>> = std::sync::Mutex::new(Vec::new());

    struct LaterWorker;

    #[async_trait]
    impl BackgroundWorker<Args> for LaterWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        async fn perform(&self, args: Args) -> Result<()> {
            if args.fail {
                return Err(Error::Worker("told to fail".to_string()));
            }
            LATER.fetch_add(args.amount, Ordering::SeqCst);
            Ok(())
        }

        async fn perform_for(&self, tenant: Option<String>, args: Args) -> Result<()> {
            LATER_TENANTS.lock().unwrap().push(tenant);
            self.perform(args).await
        }
    }

    /// Wait up to two seconds for `done` to hold.
    async fn eventually(done: impl Fn() -> bool) -> bool {
        for _ in 0_i32..200_i32 {
            if done() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn async_jobs_return_at_once_and_run_after_their_delay() {
        let ctx = context("BackgroundAsync");
        let delayed = LaterWorker::perform_in(
            &ctx,
            Args {
                amount: 5,
                fail: false,
            },
            Duration::from_millis(200),
        )
        .await
        .unwrap();
        assert!(Uuid::parse_str(&delayed).is_ok(), "{delayed}");
        assert_eq!(LATER.load(Ordering::SeqCst), 0, "not before its delay");
        assert!(eventually(|| LATER.load(Ordering::SeqCst) == 5).await);

        // A failure is logged, not returned: the caller has already moved on.
        LaterWorker::perform_later(
            &ctx,
            Args {
                amount: 0,
                fail: true,
            },
        )
        .await
        .unwrap();

        LaterWorker::perform_later_for(
            &ctx,
            "acme",
            Args {
                amount: 7,
                fail: false,
            },
        )
        .await
        .unwrap();
        assert!(eventually(|| LATER.load(Ordering::SeqCst) == 12).await);
        assert_eq!(
            *LATER_TENANTS.lock().unwrap(),
            vec![Some("acme".to_string())]
        );
        LaterWorker::perform_later_for(
            &ctx,
            "acme",
            Args {
                amount: 0,
                fail: true,
            },
        )
        .await
        .unwrap();
        assert!(eventually(|| LATER_TENANTS.lock().unwrap().len() == 2).await);
    }
}
