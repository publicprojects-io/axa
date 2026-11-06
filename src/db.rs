//! Database connection helpers.

#[cfg(feature = "sea-orm")]
pub use axa_seaorm::truncate_table;

#[cfg(any(feature = "sea-orm", feature = "sqlx"))]
use crate::{Result, config::Database};

/// Connect a pool from configuration.
///
/// # Errors
///
/// Fails when the database cannot be reached.
#[cfg(feature = "sea-orm")]
pub async fn connect(config: &Database) -> Result<sea_orm::DatabaseConnection> {
    let pool = axa_seaorm::PoolConfig {
        uri: config.uri.clone(),
        min_connections: config.min_connections,
        max_connections: config.max_connections,
        connect_timeout_ms: config.connect_timeout,
        idle_timeout_ms: config.idle_timeout,
        enable_logging: config.enable_logging,
    };
    Ok(axa_seaorm::connect(&pool).await?)
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

/// Connect a pool from configuration.
///
/// # Errors
///
/// Fails when the database cannot be reached.
#[cfg(all(feature = "sqlx", not(feature = "sea-orm")))]
pub async fn connect(config: &Database) -> Result<axa_sqlx::sqlx::PgPool> {
    Ok(axa_sqlx::connect(&config.into()).await?)
}
