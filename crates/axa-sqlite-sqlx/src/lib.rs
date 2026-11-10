//! `SQLite` pooling, migrations, and test resets over sqlx.
//!
//! This crate holds the operations every service repeats. It does not wrap
//! queries: a service writes its own SQL with `sqlx::query!`, so a reviewer
//! reads exactly what runs.
//!
//! It has the same shape as `axa-postgres-sqlx`, `axa-postgres-seaorm`,
//! `axa-postgres-diesel`, and `axa-postgres-toasty`: a [`Pool`], a
//! [`PoolConfig`], [`connect`], [`ping`], [`migrate`], [`revert`], [`pending`],
//! [`truncate`], [`dangerously_recreate`], [`is_unique_violation`], and
//! [`quote_identifier`], with [`status`] besides. What differs for `SQLite`:
//!
//! - A URL is `sqlite://path` or `sqlite::memory:`, and anything not
//!   starting with `sqlite:` is refused. A missing file is not
//!   created unless the URL asks with `?mode=rwc`, so a mistyped path fails
//!   rather than opening an empty database.
//! - Every connection is opened with `PRAGMA foreign_keys = ON`, a busy
//!   timeout ([`PoolConfig::busy_timeout_ms`], five seconds by default), and
//!   WAL. An in-memory database cannot use WAL, so `SQLite` keeps its own
//!   memory journal there. One `sqlite::memory:` pool is one database, shared
//!   by its connections, and it is gone when the pool closes.
//! - [`migrate`] takes no lock, because sqlx's `SQLite` migrator has none.
//!   Each migration runs in its own transaction, so a migration is never
//!   applied twice, but two processes migrating one file at the same moment
//!   can see one of them fail. Migrate from one process at a time.
//! - [`truncate`] is `DELETE FROM` each table, with autoincrement counters
//!   cleared, in one transaction. `SQLite` has no `TRUNCATE ... CASCADE`: a
//!   table still referenced from a table not listed, without `ON DELETE
//!   CASCADE`, makes the call fail and change nothing.
//! - [`dangerously_recreate`] drops every table and view in `sqlite_schema`
//!   other than `SQLite`'s own. There are no enum types or extensions to
//!   consider.
//! - There is no network, so no TLS.
//!
//! ```no_run
//! use axa_sqlite_sqlx::{PoolConfig, connect, migrate, sqlx};
//!
//! static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate::Migrator::DEFAULT;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = connect(&PoolConfig::new("sqlite://app.db")).await?;
//! migrate(&pool, &MIGRATOR).await?;
//! # Ok(())
//! # }
//! ```

use std::{
    ops::{Deref, DerefMut},
    time::Duration,
};

pub use sqlx;
use sqlx::{
    AssertSqlSafe, Row, Sqlite, SqliteConnection, SqlitePool,
    migrate::{MigrateError, Migrator},
    pool::PoolConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

/// The handle a service holds: the pool.
pub type Pool = SqlitePool;

/// What this crate's connection and query functions return on failure.
pub type Error = sqlx::Error;

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
    /// How long a connection waits for another's write lock, in milliseconds.
    pub busy_timeout_ms: u64,
}

impl std::fmt::Debug for PoolConfig {
    /// Leaves the connection string out, as every crate of this shape does,
    /// because for other databases it can carry a password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("min_connections", &self.min_connections)
            .field("max_connections", &self.max_connections)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("busy_timeout_ms", &self.busy_timeout_ms)
            .finish_non_exhaustive()
    }
}

impl PoolConfig {
    /// Settings for `uri` with a small pool and a five-second busy timeout.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            min_connections: 1,
            max_connections: 10,
            connect_timeout_ms: 5_000,
            idle_timeout_ms: 600_000,
            busy_timeout_ms: 5_000,
        }
    }

    /// The sqlx pool options these settings describe.
    #[must_use]
    pub fn options(&self) -> SqlitePoolOptions {
        SqlitePoolOptions::new()
            .min_connections(self.min_connections)
            .max_connections(self.max_connections)
            .acquire_timeout(Duration::from_millis(self.connect_timeout_ms))
            .idle_timeout(Duration::from_millis(self.idle_timeout_ms))
    }

    /// The options each connection is opened with: the URL, with foreign
    /// keys on, WAL, and the busy timeout.
    ///
    /// Each call to this for `sqlite::memory:` names a new in-memory database.
    ///
    /// # Errors
    ///
    /// Fails when the URL does not start with `sqlite:` or cannot be read.
    /// Sqlx alone would take any other text as a file name.
    pub fn connect_options(&self) -> Result<SqliteConnectOptions, sqlx::Error> {
        if !self.uri.starts_with("sqlite:") {
            return Err(sqlx::Error::Configuration(
                "a SQLite URL starts with `sqlite:`".into(),
            ));
        }
        Ok(self
            .uri
            .parse::<SqliteConnectOptions>()?
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(self.busy_timeout_ms)))
    }
}

/// Open a pool and prove it works with one connection.
///
/// # Errors
///
/// Fails when the URL cannot be read or the database cannot be opened.
pub async fn connect(config: &PoolConfig) -> Result<SqlitePool, sqlx::Error> {
    config
        .options()
        .connect_with(config.connect_options()?)
        .await
}

/// Check that the database answers.
///
/// # Errors
///
/// Fails when it does not.
pub async fn ping(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

/// Where a function gets its connection: the pool, or a connection the caller
/// holds, usually an open transaction, so one function serves both.
///
/// `impl sqlx::Acquire` says the same, but its associated types stop the future
/// being `Send`, which Axum handlers and `tokio::spawn` need.
///
/// ```no_run
/// use axa_sqlite_sqlx::{Db, sqlx};
///
/// async fn count(db: impl Into<Db<'_>>) -> Result<i64, sqlx::Error> {
///     let mut conn = db.into().acquire().await?;
///     sqlx::query_scalar("SELECT count(*) FROM items")
///         .fetch_one(&mut *conn)
///         .await
/// }
/// ```
pub enum Db<'a> {
    /// Take a connection from the pool for the length of the call.
    Pool(&'a SqlitePool),
    /// Use the caller's connection.
    Conn(&'a mut SqliteConnection),
}

impl<'a> From<&'a SqlitePool> for Db<'a> {
    fn from(pool: &'a SqlitePool) -> Self {
        Self::Pool(pool)
    }
}

impl<'a> From<&'a mut SqliteConnection> for Db<'a> {
    fn from(conn: &'a mut SqliteConnection) -> Self {
        Self::Conn(conn)
    }
}

impl<'a> Db<'a> {
    /// The connection to use.
    ///
    /// # Errors
    ///
    /// Fails when the pool has none to give.
    pub async fn acquire(self) -> Result<Conn<'a>, sqlx::Error> {
        match self {
            Self::Pool(pool) => Ok(Conn::Pooled(pool.acquire().await?)),
            Self::Conn(conn) => Ok(Conn::Borrowed(conn)),
        }
    }
}

/// A connection for the length of one call. It derefs to [`SqliteConnection`].
pub enum Conn<'a> {
    /// Taken from the pool, and given back when dropped.
    Pooled(PoolConnection<Sqlite>),
    /// The caller's.
    Borrowed(&'a mut SqliteConnection),
}

impl Deref for Conn<'_> {
    type Target = SqliteConnection;

    fn deref(&self) -> &SqliteConnection {
        match self {
            Self::Pooled(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

impl DerefMut for Conn<'_> {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        match self {
            Self::Pooled(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

/// Whether an error is a violated unique constraint, including a duplicate
/// primary key, which a service usually reports as `409` rather than `500`.
#[must_use]
pub fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
}

/// Quote a table name for use in SQL, in double quotes.
///
/// Identifiers cannot be bound as parameters, so this is the one place names
/// reach SQL as text. Only lowercase ASCII letters, digits, and underscores
/// are accepted, starting with a letter or underscore. That is stricter than
/// `SQLite`, and it leaves nothing for an attacker to work with.
///
/// ```
/// assert_eq!(axa_sqlite_sqlx::quote_identifier("users").as_deref(), Some("\"users\""));
/// assert_eq!(axa_sqlite_sqlx::quote_identifier("users; DROP TABLE x"), None);
/// ```
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("\"{name}\""))
}

/// Quote every name, failing on the first one [`quote_identifier`] refuses.
fn quote_all<S: AsRef<str>>(names: &[S]) -> Result<Vec<String>, sqlx::Error> {
    names
        .iter()
        .map(|name| {
            quote_identifier(name.as_ref()).ok_or_else(|| {
                sqlx::Error::Protocol(format!("invalid identifier: {}", name.as_ref()))
            })
        })
        .collect()
}

/// Whether a table of this name exists.
async fn table_exists(conn: &mut SqliteConnection, name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
    )
    .bind(name)
    .fetch_one(conn)
    .await
}

/// Apply pending migrations, each in its own transaction.
///
/// Sqlx's `SQLite` migrator takes no lock, so unlike Postgres two processes
/// migrating one file at once do not take turns, and one of them can fail.
///
/// # Errors
///
/// Fails when a migration fails, or an applied one no longer matches.
pub async fn migrate(pool: &SqlitePool, migrator: &Migrator) -> Result<(), MigrateError> {
    migrator.run(pool).await
}

/// The applied versions, newest first. Empty on a database that has never
/// been migrated.
async fn applied_versions(pool: &SqlitePool) -> Result<Vec<i64>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    if !table_exists(&mut conn, "_sqlx_migrations").await? {
        return Ok(Vec::new());
    }
    let rows =
        sqlx::query("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version DESC")
            .fetch_all(&mut *conn)
            .await?;
    rows.iter().map(|row| row.try_get("version")).collect()
}

/// Revert the newest `steps` migrations. Each needs a `.down.sql` file.
///
/// # Errors
///
/// Fails on a database error or a failed down migration.
pub async fn revert(
    pool: &SqlitePool,
    migrator: &Migrator,
    steps: u32,
) -> Result<(), MigrateError> {
    let applied = applied_versions(pool).await?;
    let steps = usize::try_from(steps).unwrap_or(usize::MAX);
    // `undo` reverts everything newer than its target, so the target is the
    // version just past the last one to revert, or 0 to revert them all.
    let target = applied.get(steps).copied().unwrap_or(0);
    migrator.undo(pool, target).await
}

/// One migration and whether it has run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Its version.
    pub version: i64,
    /// Its description.
    pub description: String,
    /// Whether it has been applied.
    pub applied: bool,
}

/// Every migration in `migrator`, oldest first, with whether it has run.
///
/// # Errors
///
/// Fails on a database error.
pub async fn status(
    pool: &SqlitePool,
    migrator: &Migrator,
) -> Result<Vec<MigrationStatus>, sqlx::Error> {
    let applied = applied_versions(pool).await?;
    Ok(migrator
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| MigrationStatus {
            version: migration.version,
            description: migration.description.to_string(),
            applied: applied.contains(&migration.version),
        })
        .collect())
}

/// The names of the migrations in `migrator` not yet applied, in order, each as
/// `version_description`.
///
/// # Errors
///
/// Fails on a database error.
pub async fn pending(pool: &SqlitePool, migrator: &Migrator) -> Result<Vec<String>, sqlx::Error> {
    Ok(status(pool, migrator)
        .await?
        .into_iter()
        .filter(|migration| !migration.applied)
        .map(|migration| format!("{}_{}", migration.version, migration.description))
        .collect())
}

/// Empty `tables`, restarting their autoincrement counters. Each name goes
/// through [`quote_identifier`], and every name is checked before anything
/// is emptied.
///
/// It runs `DELETE FROM` each table, and clears their rows in
/// `sqlite_sequence` if that exists, in one transaction. Foreign keys are
/// checked when it commits, so the tables can be listed in any order. A table
/// still referenced from a table not listed makes the call fail and change
/// nothing, unless the reference says `ON DELETE CASCADE`.
///
/// # Errors
///
/// Fails on an invalid name, a foreign key left dangling, or a database error.
pub async fn truncate<S: AsRef<str>>(pool: &SqlitePool, tables: &[S]) -> Result<(), sqlx::Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let quoted = quote_all(tables)?;
    let mut tx = pool.begin().await?;
    // Lasts until the transaction ends.
    sqlx::query("PRAGMA defer_foreign_keys = ON")
        .execute(&mut *tx)
        .await?;
    for table in &quoted {
        let sql = format!("DELETE FROM {table}");
        sqlx::query(AssertSqlSafe(sql)).execute(&mut *tx).await?;
    }
    if table_exists(&mut tx, "sqlite_sequence").await? {
        for table in tables {
            sqlx::query("DELETE FROM sqlite_sequence WHERE name = ?1")
                .bind(table.as_ref())
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await
}

/// Drop every table and view other than `SQLite`'s own, with foreign keys
/// off, then migrate from nothing. This gives each test an empty schema,
/// whatever tables it touched.
///
/// **This destroys all data.** It exists for tests, and a service must never
/// reach it outside the `test` environment.
///
/// The drops run in one transaction on one connection, which has foreign keys
/// turned back on before it returns to the pool. Indexes and triggers go with
/// their tables.
///
/// # Errors
///
/// Fails on a database error, or a name [`quote_identifier`] refuses.
pub async fn dangerously_recreate(
    pool: &SqlitePool,
    migrator: &Migrator,
) -> Result<(), MigrateError> {
    let mut conn = pool.acquire().await?;
    // `foreign_keys` cannot change inside a transaction, so it is set first.
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await?;
    let dropped = drop_everything(&mut conn).await;
    let restored = sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await;
    if let Err(error) = restored {
        // Never give the pool a connection that might not check foreign keys.
        drop(conn.detach());
        return Err(error.into());
    }
    drop(conn);
    dropped?;
    migrate(pool, migrator).await
}

/// Drop every view, then every table, that `SQLite` did not make.
async fn drop_everything(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let objects: Vec<(String, String)> = sqlx::query_as(
        "SELECT type, name FROM sqlite_schema
         WHERE type IN ('view', 'table') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
         ORDER BY type DESC",
    )
    .fetch_all(&mut *conn)
    .await?;
    let names: Vec<&str> = objects.iter().map(|(_, name)| name.as_str()).collect();
    let quoted = quote_all(&names)?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    for ((kind, _), name) in objects.iter().zip(quoted) {
        let sql = if kind == "view" {
            format!("DROP VIEW {name}")
        } else {
            format!("DROP TABLE {name}")
        };
        sqlx::query(AssertSqlSafe(sql)).execute(&mut *tx).await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("sqlite:///srv/hunter2/app.db"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
        assert!(text.contains("busy_timeout_ms"), "{text}");
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_identifier("users").as_deref(), Some("\"users\""));
        assert_eq!(quote_identifier("_a1").as_deref(), Some("\"_a1\""));
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a\"b", "a;b", "a.b", "users--", "é", "a`b", "[a]",
        ] {
            assert_eq!(quote_identifier(name), None, "{name:?}");
        }
    }

    #[test]
    fn quote_all_fails_on_any_bad_name() {
        assert!(quote_all(&["good", "also_good"]).is_ok());
        assert!(quote_all(&["good", "bad name"]).is_err());
    }

    #[test]
    fn pool_settings_reach_the_options() {
        let config = PoolConfig {
            min_connections: 2,
            max_connections: 7,
            connect_timeout_ms: 1_500,
            idle_timeout_ms: 9_000,
            ..PoolConfig::new("sqlite::memory:")
        };
        let options = config.options();
        assert_eq!(options.get_min_connections(), 2);
        assert_eq!(options.get_max_connections(), 7);
        assert_eq!(options.get_acquire_timeout(), Duration::from_millis(1_500));
        assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(9)));
    }

    #[test]
    fn a_url_that_is_not_sqlite_is_refused() {
        assert!(PoolConfig::new("postgres://x").connect_options().is_err());
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&sqlx::Error::RowNotFound));
    }
}
