//! The state every handler receives.

use std::sync::Arc;

use crate::{Environment, config::Config, store::Store};

/// Database handle held by [`AppContext`].
#[cfg(feature = "sea-orm")]
pub type Db = sea_orm::DatabaseConnection;
/// Database handle held by [`AppContext`]; none without a database feature.
#[cfg(not(feature = "sea-orm"))]
pub type Db = ();

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
    /// The job queue, when one is configured.
    #[cfg(feature = "queue")]
    pub queue_provider: Option<Arc<crate::queue::Queue>>,
}

impl AppContext {
    /// Build a context with an empty store and no queue.
    #[must_use]
    pub fn new(environment: Environment, db: Db, config: Config) -> Self {
        Self {
            environment,
            db,
            config: Arc::new(config),
            shared_store: Arc::new(Store::default()),
            #[cfg(feature = "queue")]
            queue_provider: None,
        }
    }
}
