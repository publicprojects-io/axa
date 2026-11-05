//! `SeaORM` connections, migration helpers, and model errors.
//!
//! This crate makes leaving Loco easy. A project keeps `SeaORM` and most of its
//! code, because the helpers Loco provided keep their names and behaviour:
//!
//! | Loco | Here |
//! |---|---|
//! | `loco_rs::model::{ModelError, ModelResult}` | [`model`] |
//! | `loco_rs::model::query::condition()` | [`query::condition`] |
//! | `loco_rs::schema` | [`schema`] |
//! | `loco_rs::validation::Validatable` | [`validation::Validatable`] |
//! | `loco_rs::db::truncate_table` | [`truncate_table`] |
//!
//! ```no_run
//! use axa_seaorm::{PoolConfig, connect};
//!
//! # async fn run() -> Result<(), axa_seaorm::sea_orm::DbErr> {
//! let db = connect(&PoolConfig::new("postgres://localhost/app")).await?;
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

pub use sea_orm;
use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr, EntityTrait};
pub use sea_orm_migration;

pub mod model;
pub mod query;
pub mod schema;
pub mod validation;

/// Pool settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// Connection string.
    pub uri: String,
    /// Connections kept open.
    pub min_connections: u32,
    /// Largest number of connections.
    pub max_connections: u32,
    /// How long to wait for a connection, in milliseconds.
    pub connect_timeout_ms: u64,
    /// How long an idle connection stays open, in milliseconds.
    pub idle_timeout_ms: u64,
    /// Log every statement.
    pub enable_logging: bool,
}

impl PoolConfig {
    /// Settings for `uri` with a small pool.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            min_connections: 1,
            max_connections: 10,
            connect_timeout_ms: 500,
            idle_timeout_ms: 500,
            enable_logging: false,
        }
    }

    /// The `SeaORM` options these settings describe.
    #[must_use]
    pub fn options(&self) -> ConnectOptions {
        let mut options = ConnectOptions::new(self.uri.clone());
        options
            .min_connections(self.min_connections)
            .max_connections(self.max_connections)
            .connect_timeout(Duration::from_millis(self.connect_timeout_ms))
            .idle_timeout(Duration::from_millis(self.idle_timeout_ms))
            .sqlx_logging(self.enable_logging);
        options
    }
}

/// Open a pool.
///
/// # Errors
///
/// Fails when the database cannot be reached.
pub async fn connect(config: &PoolConfig) -> Result<DatabaseConnection, DbErr> {
    Database::connect(config.options()).await
}

/// Delete every row of one entity's table. Rows in other tables are left
/// alone, so a test that needs a truly empty database should use
/// `Migrator::fresh` instead.
///
/// # Errors
///
/// Propagates the database error.
pub async fn truncate_table<T: EntityTrait>(
    db: &DatabaseConnection,
    _entity: T,
) -> Result<(), DbErr> {
    T::delete_many().exec(db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_settings_reach_the_options() {
        let config = PoolConfig {
            min_connections: 2,
            max_connections: 7,
            connect_timeout_ms: 1_500,
            idle_timeout_ms: 9_500,
            ..PoolConfig::new("postgres://x")
        };
        let options = config.options();
        assert_eq!(options.get_min_connections(), Some(2));
        assert_eq!(options.get_max_connections(), Some(7));
        assert_eq!(
            options.get_connect_timeout(),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(
            options.get_idle_timeout(),
            Some(Some(Duration::from_millis(9_500)))
        );
    }
}
