//! Database connections from configuration, for whichever database feature is on.
//!
//! Each database crate has the same shape: a `Pool`, a `PoolConfig`, `connect`,
//! `ping`, `migrate`, `pending`, `truncate`, `dangerously_recreate`,
//! `is_unique_violation`, and `quote_identifier`, and `revert` where its migrations
//! have down steps. Here, each `PoolConfig` is built from [`Database`], and
//! [`connect`] opens the handle [`crate::AppContext`] holds.

#[cfg(feature = "sea-orm")]
pub use axa_seaorm::truncate_table;

#[cfg(not(axa_db = "none"))]
use crate::Result;
#[cfg(any(
    feature = "sea-orm",
    feature = "sqlx",
    feature = "diesel",
    feature = "toasty"
))]
use crate::config::Database;

/// The pool settings in the configuration.
#[cfg(feature = "sea-orm")]
impl From<&Database> for axa_seaorm::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
            enable_logging: config.enable_logging,
        }
    }
}

/// The pool settings in the configuration.
#[cfg(feature = "sqlx")]
impl From<&Database> for axa_sqlx::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
        }
    }
}

/// The pool settings in the configuration. Diesel's pool keeps no minimum, and
/// closes no idle connection, so it reads the most connections and the wait.
#[cfg(feature = "diesel")]
impl From<&Database> for axa_diesel::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            max_connections: usize::try_from(config.max_connections).unwrap_or(usize::MAX),
            wait_timeout: std::time::Duration::from_millis(config.connect_timeout),
        }
    }
}

/// The pool settings in the configuration. Toasty's pool keeps no minimum.
#[cfg(feature = "toasty")]
impl From<&Database> for axa_toasty::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
        }
    }
}

/// Open the handle [`crate::AppContext`] holds, from configuration.
///
/// # Errors
///
/// Fails when the database cannot be reached.
#[cfg(any(axa_db = "sqlx", axa_db = "seaorm", axa_db = "diesel"))]
pub async fn connect(config: &Database) -> Result<crate::context::Db> {
    #[cfg(axa_db = "sqlx")]
    let db = axa_sqlx::connect(&config.into()).await?;
    #[cfg(axa_db = "seaorm")]
    let db = axa_seaorm::connect(&config.into()).await?;
    #[cfg(axa_db = "diesel")]
    let db = axa_diesel::connect(&config.into()).await?;
    Ok(db)
}

/// Open the handle [`crate::AppContext`] holds, from configuration, for the models
/// Toasty is to know, such as `toasty::models!(User, Post)`.
///
/// # Errors
///
/// Fails when the database cannot be reached.
#[cfg(axa_db = "toasty")]
pub async fn connect(
    config: &Database,
    models: axa_toasty::toasty::ModelSet,
) -> Result<crate::context::Db> {
    Ok(axa_toasty::connect(&config.into(), models).await?)
}

#[cfg(all(test, feature = "sqlx"))]
mod debug_tests {
    #[tokio::test]
    async fn a_pools_debug_text_holds_no_password() {
        let pool = sqlx::PgPool::connect_lazy("postgres://me:hunter2@db.internal/app").unwrap();
        let printed = format!("{pool:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
    }
}
