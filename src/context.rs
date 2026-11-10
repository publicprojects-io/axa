//! The state every handler receives.

use std::sync::Arc;

use crate::{Environment, config::Config, health::Checks, store::Store};

/// The database handle [`AppContext`] holds: `sqlx`'s pool with the `sqlx` feature,
/// `SeaORM`'s connection with `sea-orm`, a Diesel pool with `diesel`, and Toasty's
/// handle with `toasty`. With more than one on, `sqlx` wins, then `sea-orm`, `diesel`,
/// and `toasty`. With none, there is no database.
#[cfg(axa_db = "sqlx")]
pub type Db = axa_sqlx::Pool;
/// The database handle [`AppContext`] holds: `SeaORM`'s connection.
#[cfg(axa_db = "seaorm")]
pub type Db = axa_seaorm::Pool;
/// The database handle [`AppContext`] holds: a Diesel pool.
#[cfg(axa_db = "diesel")]
pub type Db = axa_diesel::Pool;
/// The database handle [`AppContext`] holds: Toasty's handle.
#[cfg(axa_db = "toasty")]
pub type Db = axa_toasty::Pool;
/// The database handle [`AppContext`] holds; none without a database feature.
#[cfg(axa_db = "none")]
pub type Db = ();

/// Check that the database answers, whichever it is.
///
/// # Errors
///
/// Fails when the database does not answer.
#[cfg(not(axa_db = "none"))]
pub async fn ping(db: &Db) -> crate::Result<()> {
    #[cfg(axa_db = "sqlx")]
    axa_sqlx::ping(db).await?;
    #[cfg(axa_db = "seaorm")]
    axa_seaorm::ping(db).await?;
    #[cfg(axa_db = "diesel")]
    axa_diesel::ping(db).await?;
    #[cfg(axa_db = "toasty")]
    axa_toasty::ping(db).await?;
    Ok(())
}

/// A database handle that was never opened, so that a test can build a context.
/// A sqlx pool needs a runtime to exist, even one that never connects.
#[cfg(all(test, axa_db = "seaorm"))]
fn test_db() -> Db {
    sea_orm::DatabaseConnection::default()
}
#[cfg(all(test, axa_db = "sqlx"))]
fn test_db() -> Db {
    static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
        std::sync::LazyLock::new(|| tokio::runtime::Runtime::new().expect("a runtime"));
    let _entered = RUNTIME.enter();
    axa_sqlx::sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://axa:axa@127.0.0.1:1/axa")
        .expect("a well-formed address")
}
#[cfg(all(test, axa_db = "diesel"))]
fn test_db() -> Db {
    let config = axa_diesel::PoolConfig {
        wait_timeout: std::time::Duration::from_millis(50),
        ..axa_diesel::PoolConfig::new("postgres://axa:axa@127.0.0.1:1/axa")
    };
    let tls = axa_diesel::web_tls().expect("TLS settings");
    axa_diesel::pool_with_tls(&config, tls).expect("a pool that never connects")
}
#[cfg(all(test, axa_db = "toasty"))]
fn test_db() -> Db {
    unimplemented!(
        "Toasty connects when its handle is made, so a unit test with `toasty` as the only \
         database feature has no context"
    )
}
#[cfg(all(test, axa_db = "none"))]
fn test_db() -> Db {}

/// Shared service state, used as ordinary Axum state with
/// `State(ctx): State<AppContext>`.
#[derive(Clone)]
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

/// The database is named by its type, as a Diesel pool has no `Debug` text, and
/// none of them would add anything a reader needs.
impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("AppContext");
        debug
            .field("environment", &self.environment)
            .field("db", &std::any::type_name::<Db>())
            .field("config", &self.config)
            .field("shared_store", &self.shared_store)
            .field("health_checks", &self.health_checks);
        #[cfg(feature = "queue")]
        debug.field("queue_provider", &self.queue_provider);
        debug.finish()
    }
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
