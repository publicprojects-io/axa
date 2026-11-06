//! The state every handler receives.

use std::sync::Arc;

use crate::{Environment, config::Config, health::Checks, store::Store};

/// Database handle held by [`AppContext`].
#[cfg(feature = "sea-orm")]
pub type Db = sea_orm::DatabaseConnection;
/// Database handle held by [`AppContext`]: the pool, with the `sqlx` feature.
#[cfg(all(feature = "sqlx", not(feature = "sea-orm")))]
pub type Db = axa_sqlx::sqlx::PgPool;
/// Database handle held by [`AppContext`]; none without a database feature.
#[cfg(not(any(feature = "sea-orm", feature = "sqlx")))]
pub type Db = ();

/// A database handle that was never opened, so that a test can build a context.
/// A sqlx pool needs a runtime to exist, even one that never connects.
#[cfg(all(test, feature = "sea-orm"))]
fn test_db() -> Db {
    sea_orm::DatabaseConnection::default()
}
#[cfg(all(test, feature = "sqlx", not(feature = "sea-orm")))]
fn test_db() -> Db {
    static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
        std::sync::LazyLock::new(|| tokio::runtime::Runtime::new().expect("a runtime"));
    let _entered = RUNTIME.enter();
    axa_sqlx::sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://axa:axa@127.0.0.1:1/axa")
        .expect("a well-formed address")
}
#[cfg(all(test, not(any(feature = "sea-orm", feature = "sqlx"))))]
fn test_db() -> Db {}

/// Shared service state, used as ordinary Axum state with
/// `State(ctx): State<AppContext>`.
#[derive(Clone, Debug)]
pub struct AppContext {
    /// The running environment.
    pub environment: Environment,
    /// The database connection pool.
    pub db: Db,
    /// Service configuration.
    pub config: Arc<Config>,
    /// Values shared across handlers.
    pub shared_store: Arc<Store>,
    /// What `/_readiness` asks, besides the database and queue that axa
    /// knows about. Set this once, before building the routes.
    pub health_checks: Checks,
    /// The job queue, when one is configured.
    #[cfg(feature = "queue")]
    pub queue_provider: Option<Arc<crate::queue::Queue>>,
}

impl AppContext {
    /// Build a context with an empty store, no named checks, and no queue.
    #[must_use]
    pub fn new(environment: Environment, db: Db, config: Config) -> Self {
        Self {
            environment,
            db,
            config: Arc::new(config),
            shared_store: Arc::new(Store::default()),
            health_checks: Checks::new(),
            #[cfg(feature = "queue")]
            queue_provider: None,
        }
    }

    /// A context for a test, whose database was never opened.
    #[cfg(test)]
    #[allow(clippy::unit_arg, dead_code)]
    pub(crate) fn for_test(environment: Environment, config: Config) -> Self {
        Self::new(environment, test_db(), config)
    }
}
