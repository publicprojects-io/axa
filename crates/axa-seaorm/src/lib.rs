//! `SeaORM` connections, migrations, schema helpers, and test resets.
//!
//! This crate has the same shape as `axa-sqlx`, `axa-diesel`, and `axa-toasty`: a
//! [`Pool`], a [`PoolConfig`], [`connect`], [`ping`], [`migrate`], [`revert`],
//! [`pending`], [`truncate`], [`dangerously_recreate`], [`is_unique_violation`], and
//! [`quote_identifier`]. The migration functions take the service's `Migrator` as a
//! type, as `sea-orm-migration` does.
//!
//! It also makes leaving Loco easy. A project keeps `SeaORM` and most of its code,
//! because the helpers Loco provided keep their names and behaviour:
//!
//! | Loco | Here |
//! |---|---|
//! | `loco_rs::model::{ModelError, ModelResult}` | `axa::model`, for every database |
//! | `loco_rs::model::query::condition()` | [`query::condition`] |
//! | `loco_rs::schema` | [`schema`] |
//! | `loco_rs::validation::Validatable` | `axa::validation`, for every database |
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
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbErr, EntityTrait, SqlErr,
};
pub use sea_orm_migration;
use sea_orm_migration::MigratorTrait;

pub mod query;
pub mod schema;

/// The handle a service holds: `SeaORM`'s connection, which owns a pool.
pub type Pool = DatabaseConnection;

/// What this crate's functions return on failure.
pub type Error = DbErr;

/// Pool settings.
#[derive(Clone, PartialEq, Eq)]
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

impl std::fmt::Debug for PoolConfig {
    /// Leaves the connection string out, because it can carry a password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("min_connections", &self.min_connections)
            .field("max_connections", &self.max_connections)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("enable_logging", &self.enable_logging)
            .finish_non_exhaustive()
    }
}

impl PoolConfig {
    /// Settings for `uri` with a small pool.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            min_connections: 1,
            max_connections: 10,
            connect_timeout_ms: 5_000,
            idle_timeout_ms: 600_000,
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

/// Check the database connection.
///
/// # Errors
///
/// Fails when the database does not answer.
pub async fn ping(pool: &Pool) -> Result<(), DbErr> {
    pool.ping().await
}

/// Apply every pending migration of `M`.
///
/// # Errors
///
/// Fails on a database error, and then no later migration is tried.
pub async fn migrate<M: MigratorTrait>(pool: &Pool) -> Result<(), DbErr> {
    M::up(pool, None).await
}

/// Revert the last `steps` applied migrations of `M`, newest first.
///
/// # Errors
///
/// Fails on a database error.
pub async fn revert<M: MigratorTrait>(pool: &Pool, steps: u32) -> Result<(), DbErr> {
    M::down(pool, Some(steps)).await
}

/// The names of the migrations of `M` not yet applied, in order.
///
/// # Errors
///
/// Fails on a database error.
pub async fn pending<M: MigratorTrait>(pool: &Pool) -> Result<Vec<String>, DbErr> {
    Ok(M::get_pending_migrations(pool)
        .await?
        .iter()
        .map(|migration| migration.name().to_string())
        .collect())
}

/// Drop every table, then apply every migration of `M` from the start, as
/// `Migrator::fresh` does. This destroys data: `axa::cli` allows it only in
/// development and test, and only when confirmed.
///
/// # Errors
///
/// Fails on a database error.
pub async fn dangerously_recreate<M: MigratorTrait>(pool: &Pool) -> Result<(), DbErr> {
    M::fresh(pool).await
}

/// Whether `error` is the database refusing a duplicate under a unique constraint.
#[must_use]
pub fn is_unique_violation(error: &DbErr) -> bool {
    matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

/// `name` in double quotes, when it is a plain lower-case identifier: a letter or
/// `_`, then letters, digits, and `_`. Anything else is refused, so it cannot carry
/// SQL.
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("\"{name}\""))
}

/// Empty `tables` and restart their identities, for a test. Every name must be a
/// plain lower-case identifier. [`truncate_table`] empties one entity's table.
///
/// # Errors
///
/// Fails on a database error, or a name that is not a plain identifier, before
/// anything is emptied.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), DbErr> {
    if tables.is_empty() {
        return Ok(());
    }
    let tables = tables
        .iter()
        .map(|name| {
            quote_identifier(name.as_ref())
                .ok_or_else(|| DbErr::Custom(format!("invalid identifier: {}", name.as_ref())))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    pool.execute_unprepared(&format!("TRUNCATE {tables} RESTART IDENTITY CASCADE"))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_and_unsafe_ones_refused() {
        assert_eq!(quote_identifier("users").as_deref(), Some("\"users\""));
        for name in ["", "1users", "Users", "a b", "a\"b", "a;b", "a.b"] {
            assert_eq!(quote_identifier(name), None, "{name:?}");
        }
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&DbErr::Custom("nothing".to_string())));
    }

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("postgres://me:hunter2@db/app"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
    }

    #[test]
    fn the_default_timeouts_suit_a_real_database() {
        let config = PoolConfig::new("postgres://x");
        assert_eq!(config.connect_timeout_ms, 5_000);
        assert_eq!(config.idle_timeout_ms, 600_000);
    }

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
