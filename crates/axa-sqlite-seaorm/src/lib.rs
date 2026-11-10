//! `SeaORM` connections, migrations, schema helpers, and test resets, on SQLite.
//!
//! This crate is the SQLite twin of `axa-postgres-seaorm`, and has the same shape as
//! the other axa database crates: a [`Pool`], a [`PoolConfig`], [`connect`],
//! [`ping`], [`migrate`], [`revert`], [`pending`], [`truncate`],
//! [`dangerously_recreate`], [`is_unique_violation`], and [`quote_identifier`]. The
//! migration functions take the service's `Migrator` as a type, as
//! `sea-orm-migration` does.
//!
//! What differs for SQLite:
//!
//! - URLs are `sqlite://path` or `sqlite::memory:`. A file that does not exist is not
//!   made unless the URL asks for it with `?mode=rwc`. An in-memory database is shared
//!   by the pool's connections, and is gone when the last one closes.
//! - Every connection turns on foreign keys, waits up to
//!   [`PoolConfig::busy_timeout_ms`] for a lock, and, for a file, uses WAL.
//! - [`truncate`] is `DELETE FROM` each table, in one transaction with foreign key
//!   checks deferred to its commit, and clears the tables' rows in
//!   `sqlite_sequence`, so `AUTOINCREMENT` starts again at 1. SQLite has no
//!   `TRUNCATE` and no `CASCADE`: a table whose rows are still referenced from a table
//!   not in the list makes the whole call fail, and nothing is emptied.
//! - [`dangerously_recreate`] drops every table in `sqlite_schema` other than
//!   SQLite's own, on one connection with foreign keys off and in one transaction,
//!   then migrates from the start.
//! - [`schema::add_column`] can add only a column SQLite can add to a table that
//!   exists: one that is not unique or a primary key, and, once the table has rows,
//!   is nullable or has a default. SQLite refuses others with an error. No helper
//!   here changes a column's type or nullability, because SQLite's `ALTER TABLE`
//!   cannot. A foreign key that [`schema::create_table`] makes keeps no name.
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
//! use axa_sqlite_seaorm::{PoolConfig, connect};
//!
//! # async fn run() -> Result<(), axa_sqlite_seaorm::sea_orm::DbErr> {
//! let db = connect(&PoolConfig::new("sqlite://app.db?mode=rwc")).await?;
//! # Ok(())
//! # }
//! ```

use std::{sync::Arc, time::Duration};

pub use sea_orm;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbErr, EntityTrait, RuntimeErr,
    SqlErr, TransactionTrait,
    sqlx::{self, AssertSqlSafe, SqliteConnection, sqlite::SqliteJournalMode},
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
    /// Connection string: `sqlite://path` or `sqlite::memory:`.
    pub uri: String,
    /// Connections kept open.
    pub min_connections: u32,
    /// Largest number of connections.
    pub max_connections: u32,
    /// How long to wait for a connection, in milliseconds.
    pub connect_timeout_ms: u64,
    /// How long an idle connection stays open, in milliseconds.
    pub idle_timeout_ms: u64,
    /// How long a statement waits for another connection's lock, in milliseconds.
    pub busy_timeout_ms: u64,
    /// Log every statement.
    pub enable_logging: bool,
}

impl std::fmt::Debug for PoolConfig {
    /// Leaves the connection string out, as every axa database crate does, because
    /// for other databases it can carry a password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("min_connections", &self.min_connections)
            .field("max_connections", &self.max_connections)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("busy_timeout_ms", &self.busy_timeout_ms)
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
            busy_timeout_ms: 5_000,
            enable_logging: false,
        }
    }

    /// Whether the URL names an in-memory database rather than a file.
    fn is_memory(&self) -> bool {
        self.uri.contains(":memory:") || self.uri.contains("mode=memory")
    }

    /// The `SeaORM` options these settings describe, which also set every
    /// connection's foreign keys, busy timeout, and, for a file, WAL.
    #[must_use]
    pub fn options(&self) -> ConnectOptions {
        let busy_timeout = Duration::from_millis(self.busy_timeout_ms);
        let wal = !self.is_memory();
        let mut options = ConnectOptions::new(self.uri.clone());
        options
            .min_connections(self.min_connections)
            .max_connections(self.max_connections)
            .connect_timeout(Duration::from_millis(self.connect_timeout_ms))
            .idle_timeout(Duration::from_millis(self.idle_timeout_ms))
            .sqlx_logging(self.enable_logging)
            .map_sqlx_sqlite_opts(move |sqlite| {
                let sqlite = sqlite.foreign_keys(true).busy_timeout(busy_timeout);
                if wal {
                    sqlite.journal_mode(SqliteJournalMode::Wal)
                } else {
                    sqlite
                }
            });
        options
    }
}

/// Open a pool.
///
/// # Errors
///
/// Fails when the database cannot be opened, such as a file that does not exist
/// and a URL without `?mode=rwc`.
pub async fn connect(config: &PoolConfig) -> Result<DatabaseConnection, DbErr> {
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

fn sqlx_error(error: sqlx::Error) -> DbErr {
    DbErr::Exec(RuntimeErr::SqlxError(Arc::new(error)))
}

/// Any table name in double quotes, with its own double quotes doubled. Used only
/// for names read from `sqlite_schema`, never for a caller's.
fn quote_any(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Drop every table other than SQLite's own, in one transaction on `conn`, which
/// must have foreign keys off.
async fn drop_every_table(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    sqlx::raw_sql("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    let result = async {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
        )
        .fetch_all(&mut *conn)
        .await?;
        for table in tables {
            let drop = format!("DROP TABLE {}", quote_any(&table));
            sqlx::raw_sql(AssertSqlSafe(drop))
                .execute(&mut *conn)
                .await?;
        }
        sqlx::raw_sql("COMMIT").execute(&mut *conn).await
    }
    .await;
    if result.is_err() {
        sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.ok();
    }
    result.map(|_| ())
}

/// Drop every table, then apply every migration of `M` from the start. This
/// destroys data: `axa::cli` allows it only in development and test, and only
/// when confirmed.
///
/// The tables are those in `sqlite_schema` other than SQLite's own, dropped on one
/// connection with foreign keys off, in one transaction, so a failure drops none.
/// That connection is closed rather than returned to the pool if anything fails.
///
/// # Errors
///
/// Fails on a database error.
pub async fn dangerously_recreate<M: MigratorTrait>(pool: &Pool) -> Result<(), DbErr> {
    let mut conn = pool
        .get_sqlite_connection_pool()
        .acquire()
        .await
        .map_err(sqlx_error)?;
    let dropped = async {
        sqlx::raw_sql("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await?;
        drop_every_table(&mut conn).await?;
        sqlx::raw_sql("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await
            .map(|_| ())
    }
    .await;
    if let Err(error) = dropped {
        conn.close().await.ok();
        return Err(sqlx_error(error));
    }
    drop(conn);
    M::up(pool, None).await
}

/// Whether `error` is the database refusing a duplicate under a unique constraint,
/// a primary key's included.
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

/// Empty `tables` for a test, and start their `AUTOINCREMENT` again at 1. Every
/// name must be a plain lower-case identifier. [`truncate_table`] empties one
/// entity's table.
///
/// Each table is emptied with `DELETE FROM`, in one transaction, with foreign key
/// checks deferred to its commit, so the tables can be listed in any order. A row
/// left in another table that still refers to an emptied one fails the commit, and
/// then nothing is emptied.
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
        .collect::<Result<Vec<_>, _>>()?;
    let txn = pool.begin().await?;
    txn.execute_unprepared("PRAGMA defer_foreign_keys = ON")
        .await?;
    for table in &tables {
        txn.execute_unprepared(&format!("DELETE FROM {table}"))
            .await?;
    }
    let has_sequence = txn
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'sqlite_sequence'",
        ))
        .await?
        .is_some();
    if has_sequence {
        // A quoted plain identifier is also a safe string literal once its double
        // quotes become single ones.
        let names = tables
            .iter()
            .map(|table| table.replace('"', "'"))
            .collect::<Vec<_>>()
            .join(", ");
        txn.execute_unprepared(&format!(
            "DELETE FROM sqlite_sequence WHERE name IN ({names})"
        ))
        .await?;
    }
    txn.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_and_unsafe_ones_refused() {
        assert_eq!(quote_identifier("users").as_deref(), Some("\"users\""));
        for name in ["", "1users", "Users", "a b", "a\"b", "a;b", "a.b", "a'b"] {
            assert_eq!(quote_identifier(name), None, "{name:?}");
        }
    }

    #[test]
    fn any_name_read_from_the_schema_is_quoted_safely() {
        assert_eq!(quote_any("odd \"name\""), "\"odd \"\"name\"\"\"");
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&DbErr::Custom("nothing".to_string())));
    }

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("sqlite:///srv/hunter2/app.db"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
        assert!(text.contains("busy_timeout_ms"), "{text}");
    }

    #[test]
    fn the_default_timeouts_suit_a_real_database() {
        let config = PoolConfig::new("sqlite://x");
        assert_eq!(config.connect_timeout_ms, 5_000);
        assert_eq!(config.idle_timeout_ms, 600_000);
        assert_eq!(config.busy_timeout_ms, 5_000);
    }

    #[test]
    fn in_memory_urls_are_told_from_files() {
        assert!(PoolConfig::new("sqlite::memory:").is_memory());
        assert!(PoolConfig::new("sqlite://app.db?mode=memory").is_memory());
        assert!(!PoolConfig::new("sqlite://app.db?mode=rwc").is_memory());
    }

    #[test]
    fn pool_settings_reach_the_options() {
        let config = PoolConfig {
            min_connections: 2,
            max_connections: 7,
            connect_timeout_ms: 1_500,
            idle_timeout_ms: 9_500,
            ..PoolConfig::new("sqlite://x")
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
