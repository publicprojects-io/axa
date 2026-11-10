//! `SeaORM` connections, migrations, schema helpers, and test resets, for MySQL.
//!
//! This crate has the same shape as `axa-postgres-seaorm` and the other Axa
//! database crates: a [`Pool`], a [`PoolConfig`], [`connect`], [`ping`],
//! [`migrate`], [`revert`], [`pending`], [`truncate`], [`dangerously_recreate`],
//! [`is_unique_violation`], and [`quote_identifier`]. The migration functions take
//! the service's `Migrator` as a type, as `sea-orm-migration` does. It also keeps
//! Loco's helpers, [`query::condition`], [`schema`], and [`truncate_table`].
//!
//! What differs for MySQL:
//!
//! - The URL is `mysql://user:password@host:port/database`, and [`connect`] refuses
//!   any other scheme. TLS is asked for with the query parameter `ssl-mode`, one of
//!   `DISABLED`, `PREFERRED`, `REQUIRED`, `VERIFY_CA`, or `VERIFY_IDENTITY`, which
//!   the driver, `sqlx`, reads itself. TLS is rustls with the webpki roots.
//! - [`quote_identifier`] uses backticks.
//! - [`truncate`] runs `TRUNCATE TABLE` for each table, which also resets its
//!   `AUTO_INCREMENT`, with foreign key checks off, so a table that others refer to
//!   can be emptied. MySQL has no `CASCADE`: rows that refer to an emptied table are
//!   left in place, so name those tables too.
//! - [`dangerously_recreate`] drops every base table in `DATABASE()` with foreign
//!   key checks off, then applies every migration. It refuses a connection with no
//!   database selected.
//! - Foreign key checks are turned off on one connection taken out of the pool for
//!   the purpose, and turned back on, and that connection is then closed rather
//!   than returned, so no pooled connection is ever left with them off.
//! - MySQL commits each DDL statement as it goes, so a migration that fails part
//!   way leaves the statements before the failure applied: write one change per
//!   migration where that matters.
//! - [`schema`]: a `TimestampWithTimeZone` column is a MySQL `DATETIME`, not a
//!   `TIMESTAMP`, which ends in 2038. A `DATETIME` keeps no time zone: it holds UTC
//!   because sqlx sets each connection's `time_zone` to `+00:00`, which
//!   `CURRENT_TIMESTAMP` defaults follow. `JsonBinary` is `JSON`, and a UUID is `BINARY(16)`. A foreign
//!   key name longer than 64 characters is refused by MySQL.
//!
//! ```no_run
//! use axa_mysql_seaorm::{PoolConfig, connect};
//!
//! # async fn run() -> Result<(), axa_mysql_seaorm::sea_orm::DbErr> {
//! let db = connect(&PoolConfig::new("mysql://app:secret@localhost:3306/app")).await?;
//! # Ok(())
//! # }
//! ```

use std::{sync::Arc, time::Duration};

pub use sea_orm;
use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DbBackend, DbErr, EntityTrait, RuntimeErr,
    SqlErr,
    sqlx::{self, Connection, MySqlConnection},
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
    /// Connection string, `mysql://user:password@host:port/database`, with
    /// `?ssl-mode=...` to ask for TLS.
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
/// Fails when the URL does not start with `mysql://`, or the database cannot be
/// reached, or TLS that `ssl-mode` asks for cannot be set up.
pub async fn connect(config: &PoolConfig) -> Result<DatabaseConnection, DbErr> {
    if !config.uri.starts_with("mysql://") {
        return Err(DbErr::Custom(
            "a MySQL URL starts with mysql://".to_string(),
        ));
    }
    Database::connect(config.options()).await
}

/// Delete every row of one entity's table. Rows in other tables are left
/// alone, so a test that needs a truly empty database should use
/// [`dangerously_recreate`] instead.
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
/// Fails on a database error, and then no later migration is tried. MySQL
/// commits each DDL statement as it runs, so the statements of the failing
/// migration before the failure stay applied.
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

/// Drop every base table in `DATABASE()`, with foreign key checks off, then
/// apply every migration of `M` from the start. This destroys data: `axa::cli`
/// allows it only in development and test, and only when confirmed.
///
/// # Errors
///
/// Fails on a database error, when `pool` is not a MySQL pool, or when the
/// connection has no database selected.
pub async fn dangerously_recreate<M: MigratorTrait>(pool: &Pool) -> Result<(), DbErr> {
    let mut conn = detached(pool).await?;
    let result = drop_every_table(&mut conn).await;
    let closed = conn.close().await.map_err(exec_error);
    result?;
    closed?;
    migrate::<M>(pool).await
}

async fn drop_every_table(conn: &mut MySqlConnection) -> Result<(), DbErr> {
    let database: Option<String> = sqlx::query_scalar("SELECT DATABASE()")
        .fetch_one(&mut *conn)
        .await
        .map_err(exec_error)?;
    if database.is_none() {
        return Err(DbErr::Custom(
            "no database is selected: name one in the URL".to_string(),
        ));
    }
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(TABLE_NAME AS CHAR) FROM information_schema.TABLES
         WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE'",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(exec_error)?;
    // These names come from the database itself, so any name is quoted, with a
    // backtick inside it doubled.
    let statements: Vec<String> = tables
        .iter()
        .map(|name| format!("DROP TABLE `{}`", name.replace('`', "``")))
        .collect();
    without_foreign_key_checks(conn, &statements).await
}

/// Whether `error` is the database refusing a duplicate under a unique constraint.
#[must_use]
pub fn is_unique_violation(error: &DbErr) -> bool {
    matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

/// `name` in backticks, when it is a plain lower-case identifier: a letter or
/// `_`, then letters, digits, and `_`. Anything else is refused, so it cannot carry
/// SQL.
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("`{name}`"))
}

/// Empty `tables` and reset their `AUTO_INCREMENT`, for a test, with foreign key
/// checks off. Every name must be a plain lower-case identifier. Rows in other
/// tables that refer to these are left in place. [`truncate_table`] empties one
/// entity's table.
///
/// # Errors
///
/// Fails on a database error, when `pool` is not a MySQL pool, or on a name that
/// is not a plain identifier, before anything is emptied.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), DbErr> {
    if tables.is_empty() {
        return Ok(());
    }
    let statements = tables
        .iter()
        .map(|name| {
            quote_identifier(name.as_ref())
                .map(|table| format!("TRUNCATE TABLE {table}"))
                .ok_or_else(|| DbErr::Custom(format!("invalid identifier: {}", name.as_ref())))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut conn = detached(pool).await?;
    let result = without_foreign_key_checks(&mut conn, &statements).await;
    let closed = conn.close().await.map_err(exec_error);
    result?;
    closed
}

/// One connection taken out of `pool`, which opens another in its place. It is
/// closed after use rather than returned, so a session setting changed on it
/// never reaches a pooled connection, even when the caller is cancelled.
async fn detached(pool: &Pool) -> Result<MySqlConnection, DbErr> {
    if pool.get_database_backend() != DbBackend::MySql {
        return Err(DbErr::Custom("not a MySQL pool".to_string()));
    }
    let conn = pool
        .get_mysql_connection_pool()
        .acquire()
        .await
        .map_err(|error| DbErr::Conn(RuntimeErr::SqlxError(Arc::new(error))))?;
    Ok(conn.detach())
}

/// Run `statements` in order with foreign key checks off, stopping at the first
/// failure, and turn the checks back on whether or not they succeeded.
async fn without_foreign_key_checks(
    conn: &mut MySqlConnection,
    statements: &[String],
) -> Result<(), DbErr> {
    sqlx::raw_sql("SET FOREIGN_KEY_CHECKS = 0")
        .execute(&mut *conn)
        .await
        .map_err(exec_error)?;
    let mut result = Ok(());
    for statement in statements {
        if let Err(error) = sqlx::raw_sql(sqlx::AssertSqlSafe(statement.as_str()))
            .execute(&mut *conn)
            .await
        {
            result = Err(exec_error(error));
            break;
        }
    }
    let restored = sqlx::raw_sql("SET FOREIGN_KEY_CHECKS = 1")
        .execute(&mut *conn)
        .await
        .map(|_| ())
        .map_err(exec_error);
    result.and(restored)
}

fn exec_error(error: sqlx::Error) -> DbErr {
    DbErr::Exec(RuntimeErr::SqlxError(Arc::new(error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_and_unsafe_ones_refused() {
        assert_eq!(quote_identifier("users").as_deref(), Some("`users`"));
        assert_eq!(quote_identifier("_a1").as_deref(), Some("`_a1`"));
        for name in ["", "1users", "Users", "a b", "a`b", "a\"b", "a;b", "a.b"] {
            assert_eq!(quote_identifier(name), None, "{name:?}");
        }
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&DbErr::Custom("nothing".to_string())));
    }

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("mysql://me:hunter2@db:3306/app"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
    }

    #[test]
    fn the_default_timeouts_suit_a_real_database() {
        let config = PoolConfig::new("mysql://x");
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
            ..PoolConfig::new("mysql://x")
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

    #[tokio::test]
    async fn connect_refuses_a_url_that_is_not_mysql() {
        let error = connect(&PoolConfig::new("postgres://me:hunter2@db/app"))
            .await
            .unwrap_err();
        assert!(matches!(error, DbErr::Custom(_)), "{error}");
        assert!(!error.to_string().contains("hunter2"), "{error}");
    }
}
