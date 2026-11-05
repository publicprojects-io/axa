//! Database connection helpers.

#[cfg(feature = "sea-orm")]
use crate::{Result, config::Database};

/// Connect a pool from configuration.
///
/// # Errors
///
/// Fails when the database cannot be reached.
#[cfg(feature = "sea-orm")]
pub async fn connect(config: &Database) -> Result<sea_orm::DatabaseConnection> {
    use std::time::Duration;

    let mut options = sea_orm::ConnectOptions::new(config.uri.clone());
    options
        .min_connections(config.min_connections)
        .max_connections(config.max_connections)
        .connect_timeout(Duration::from_millis(config.connect_timeout))
        .idle_timeout(Duration::from_millis(config.idle_timeout))
        .sqlx_logging(config.enable_logging);
    Ok(sea_orm::Database::connect(options).await?)
}

/// Delete every row of one entity's table.
///
/// # Errors
///
/// Propagates the database error.
#[cfg(feature = "sea-orm")]
pub async fn truncate_table<T: sea_orm::EntityTrait>(
    db: &sea_orm::DatabaseConnection,
    _entity: T,
) -> std::result::Result<(), sea_orm::DbErr> {
    T::delete_many().exec(db).await?;
    Ok(())
}
