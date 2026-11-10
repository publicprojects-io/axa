//! `MySQL` pooling, migrations, and test resets over sqlx.
//!
//! This crate holds the operations every service repeats. It does not wrap
//! queries: a service writes its own SQL with `sqlx::query!`, so a reviewer
//! reads exactly what runs.
//!
//! It has the same shape as `axa-postgres-sqlx` and the other database crates:
//! a [`Pool`], a [`PoolConfig`], [`connect`], [`ping`], [`migrate`],
//! [`revert`], [`pending`], [`truncate`], [`dangerously_recreate`],
//! [`is_unique_violation`], and [`quote_identifier`], with [`status`] and
//! [`Db`] besides.
//!
//! What differs for `MySQL`:
//!
//! - URLs are `mysql://user:password@host:port/database`. TLS is asked for
//!   with the query parameter `ssl-mode`, one of `DISABLED`, `PREFERRED` (the
//!   default), `REQUIRED`, `VERIFY_CA`, or `VERIFY_IDENTITY`, as in
//!   `mysql://app@db/app?ssl-mode=VERIFY_IDENTITY`.
//! - [`quote_identifier`] quotes with backticks.
//! - `MySQL` commits each DDL statement as it runs, so a migration is not
//!   atomic. If a migration fails after a statement that changed the schema,
//!   that change stays, and sqlx has already recorded the migration as not
//!   successful. [`migrate`] then fails with [`MigrateError::Dirty`] until the
//!   database is put right by hand, or recreated in a test. Keep each
//!   migration to one schema change where you can.
//! - [`truncate`] empties each table with foreign key checks off, because
//!   `MySQL` has no `TRUNCATE ... CASCADE`. Rows in other tables that point at
//!   an emptied table are left as they are, so empty those tables too.
//! - [`dangerously_recreate`] drops every table and view in `DATABASE()`, with
//!   foreign key checks off, then migrates.
//!
//! ```no_run
//! use axa_mysql_sqlx::{PoolConfig, connect, migrate, sqlx};
//!
//! static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate::Migrator::DEFAULT;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = connect(&PoolConfig::new("mysql://app@localhost:3306/app")).await?;
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
    AssertSqlSafe, MySql, MySqlConnection, MySqlPool, Row,
    migrate::{MigrateError, Migrator},
    mysql::MySqlPoolOptions,
    pool::PoolConnection,
};

/// The handle a service holds: the pool.
pub type Pool = MySqlPool;

/// What this crate's connection and query functions return on failure.
pub type Error = sqlx::Error;

/// Pool settings.
#[derive(Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// Connection string, such as `mysql://user:password@host:3306/app`.
    pub uri: String,
    /// Connections kept open.
    pub min_connections: u32,
    /// Largest number of connections.
    pub max_connections: u32,
    /// How long to wait for a connection, in milliseconds.
    pub connect_timeout_ms: u64,
    /// How long an idle connection stays open, in milliseconds.
    pub idle_timeout_ms: u64,
}

impl std::fmt::Debug for PoolConfig {
    /// Leaves the connection string out, because it can carry a password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("min_connections", &self.min_connections)
            .field("max_connections", &self.max_connections)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
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
        }
    }

    /// The sqlx options these settings describe.
    #[must_use]
    pub fn options(&self) -> MySqlPoolOptions {
        MySqlPoolOptions::new()
            .min_connections(self.min_connections)
            .max_connections(self.max_connections)
            .acquire_timeout(Duration::from_millis(self.connect_timeout_ms))
            .idle_timeout(Duration::from_millis(self.idle_timeout_ms))
    }
}

/// Open a pool and prove it works with one connection.
///
/// # Errors
///
/// Fails when the database cannot be reached, or the URL is not a `MySQL`
/// one, such as an unknown `ssl-mode`.
pub async fn connect(config: &PoolConfig) -> Result<MySqlPool, sqlx::Error> {
    config.options().connect(&config.uri).await
}

/// Check that the database answers.
///
/// # Errors
///
/// Fails when it does not.
pub async fn ping(pool: &MySqlPool) -> Result<(), sqlx::Error> {
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
/// use axa_mysql_sqlx::{Db, sqlx};
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
    Pool(&'a MySqlPool),
    /// Use the caller's connection.
    Conn(&'a mut MySqlConnection),
}

impl<'a> From<&'a MySqlPool> for Db<'a> {
    fn from(pool: &'a MySqlPool) -> Self {
        Self::Pool(pool)
    }
}

impl<'a> From<&'a mut MySqlConnection> for Db<'a> {
    fn from(conn: &'a mut MySqlConnection) -> Self {
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

/// A connection for the length of one call. It derefs to [`MySqlConnection`].
pub enum Conn<'a> {
    /// Taken from the pool, and given back when dropped.
    Pooled(PoolConnection<MySql>),
    /// The caller's.
    Borrowed(&'a mut MySqlConnection),
}

impl Deref for Conn<'_> {
    type Target = MySqlConnection;

    fn deref(&self) -> &MySqlConnection {
        match self {
            Self::Pooled(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

impl DerefMut for Conn<'_> {
    fn deref_mut(&mut self) -> &mut MySqlConnection {
        match self {
            Self::Pooled(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

/// Whether an error is a violated unique constraint, which a service usually
/// reports as `409` rather than `500`. This is `MySQL`'s duplicate key errors,
/// such as `1062` (`ER_DUP_ENTRY`).
#[must_use]
pub fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
}

/// Quote a table name for use in SQL, with backticks.
///
/// Identifiers cannot be bound as parameters, so this is the one place names
/// reach SQL as text. Only lowercase ASCII letters, digits, and underscores
/// are accepted, starting with a letter or underscore. That is stricter than
/// `MySQL`, and it leaves nothing for an attacker to work with.
///
/// ```
/// assert_eq!(axa_mysql_sqlx::quote_identifier("users").as_deref(), Some("`users`"));
/// assert_eq!(axa_mysql_sqlx::quote_identifier("users; DROP TABLE x"), None);
/// ```
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("`{name}`"))
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

/// Run `statements` in order on one connection with foreign key checks off,
/// then turn them back on, whether or not a statement failed. A connection
/// whose checks could not be turned back on is closed rather than returned to
/// the pool.
async fn without_foreign_key_checks(
    pool: &MySqlPool,
    statements: Vec<String>,
) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    sqlx::query("SET FOREIGN_KEY_CHECKS = 0")
        .execute(&mut *conn)
        .await?;
    let mut result = Ok(());
    for sql in statements {
        if let Err(error) = sqlx::query(AssertSqlSafe(sql)).execute(&mut *conn).await {
            result = Err(error);
            break;
        }
    }
    if let Err(error) = sqlx::query("SET FOREIGN_KEY_CHECKS = 1")
        .execute(&mut *conn)
        .await
    {
        drop(conn.detach());
        return result.and(Err(error));
    }
    result
}

/// Apply pending migrations. Sqlx holds a `GET_LOCK` lock while it does, so
/// services booting together take turns.
///
/// `MySQL` commits each DDL statement as it runs, so a migration that fails
/// partway leaves the statements before the failure applied, and the next call
/// fails with [`MigrateError::Dirty`].
///
/// # Errors
///
/// Fails when a migration fails, an applied one no longer matches, or one was
/// left dirty.
pub async fn migrate(pool: &MySqlPool, migrator: &Migrator) -> Result<(), MigrateError> {
    migrator.run(pool).await
}

/// The newest applied versions, newest first. Empty on a database that has
/// never been migrated.
async fn applied_versions(pool: &MySqlPool) -> Result<Vec<i64>, sqlx::Error> {
    let found: i64 = sqlx::query(
        "SELECT count(*) AS found FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = '_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await?
    .try_get("found")?;
    if found == 0 {
        return Ok(Vec::new());
    }
    let rows =
        sqlx::query("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version DESC")
            .fetch_all(pool)
            .await?;
    rows.iter().map(|row| row.try_get("version")).collect()
}

/// Revert the newest `steps` migrations. Each needs a `.down.sql` file.
///
/// # Errors
///
/// Fails on a database error or a failed down migration.
pub async fn revert(pool: &MySqlPool, migrator: &Migrator, steps: u32) -> Result<(), MigrateError> {
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
    /// Whether it has been applied. A migration left dirty by a failure is
    /// not.
    pub applied: bool,
}

/// Every migration in `migrator`, oldest first, with whether it has run.
///
/// # Errors
///
/// Fails on a database error.
pub async fn status(
    pool: &MySqlPool,
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
pub async fn pending(pool: &MySqlPool, migrator: &Migrator) -> Result<Vec<String>, sqlx::Error> {
    Ok(status(pool, migrator)
        .await?
        .into_iter()
        .filter(|migration| !migration.applied)
        .map(|migration| format!("{}_{}", migration.version, migration.description))
        .collect())
}

/// Empty `tables`, restarting their `AUTO_INCREMENT` counters. Each name goes
/// through [`quote_identifier`], and all are checked before anything is
/// emptied.
///
/// Each table is emptied with `TRUNCATE TABLE` on one connection with foreign
/// key checks off, so a table another references can be emptied. `MySQL` has
/// no `CASCADE` here: rows in other tables that point at an emptied table are
/// left as they are, so name those tables too. Each `TRUNCATE` commits as it
/// runs, so a failure partway leaves the tables before it empty.
///
/// # Errors
///
/// Fails on an invalid name or a database error.
pub async fn truncate<S: AsRef<str>>(pool: &MySqlPool, tables: &[S]) -> Result<(), sqlx::Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let statements = quote_all(tables)?
        .into_iter()
        .map(|table| format!("TRUNCATE TABLE {table}"))
        .collect();
    without_foreign_key_checks(pool, statements).await
}

/// Drop every table and view in the connection's database, `DATABASE()`,
/// then migrate from nothing. This gives each test an empty schema, whatever
/// tables it touched.
///
/// **This destroys all data.** It exists for tests, and a service must never
/// reach it outside the `test` environment.
///
/// Tables are dropped with foreign key checks off, so their order does not
/// matter. Stored procedures, functions, and events are left in place, so a
/// migration that creates one should drop it first with `DROP ... IF EXISTS`.
///
/// # Errors
///
/// Fails when the URL names no database, on a database error, or on a name
/// [`quote_identifier`] refuses, before anything is dropped.
pub async fn dangerously_recreate(
    pool: &MySqlPool,
    migrator: &Migrator,
) -> Result<(), MigrateError> {
    let database: Option<String> = sqlx::query("SELECT DATABASE() AS name")
        .fetch_one(pool)
        .await?
        .try_get("name")?;
    if database.is_none() {
        return Err(sqlx::Error::Protocol("no database selected".to_owned()).into());
    }
    let rows = sqlx::query(
        "SELECT CAST(table_name AS CHAR) AS name, CAST(table_type AS CHAR) AS kind
         FROM information_schema.tables WHERE table_schema = DATABASE()",
    )
    .fetch_all(pool)
    .await?;
    let mut tables = Vec::new();
    let mut views = Vec::new();
    for row in &rows {
        let name: String = row.try_get("name")?;
        let kind: String = row.try_get("kind")?;
        if kind == "VIEW" {
            views.push(name);
        } else {
            tables.push(name);
        }
    }
    let mut statements = Vec::new();
    if !views.is_empty() {
        statements.push(format!("DROP VIEW {}", quote_all(&views)?.join(", ")));
    }
    if !tables.is_empty() {
        statements.push(format!("DROP TABLE {}", quote_all(&tables)?.join(", ")));
    }
    if !statements.is_empty() {
        without_foreign_key_checks(pool, statements).await?;
    }
    migrate(pool, migrator).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("mysql://me:hunter2@db/app"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
    }

    #[test]
    fn identifiers_are_quoted_with_backticks() {
        assert_eq!(quote_identifier("users").as_deref(), Some("`users`"));
        assert_eq!(quote_identifier("_a1").as_deref(), Some("`_a1`"));
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a\"b", "a`b", "a;b", "a.b", "users--", "é",
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
            ..PoolConfig::new("mysql://x")
        };
        let options = config.options();
        assert_eq!(options.get_min_connections(), 2);
        assert_eq!(options.get_max_connections(), 7);
        assert_eq!(options.get_acquire_timeout(), Duration::from_millis(1_500));
        assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(9)));
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&sqlx::Error::RowNotFound));
    }

    #[test]
    fn ssl_mode_is_read_from_the_url() {
        use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
        for (mode, expected) in [
            ("DISABLED", MySqlSslMode::Disabled),
            ("PREFERRED", MySqlSslMode::Preferred),
            ("REQUIRED", MySqlSslMode::Required),
            ("VERIFY_CA", MySqlSslMode::VerifyCa),
            ("VERIFY_IDENTITY", MySqlSslMode::VerifyIdentity),
        ] {
            let url = format!("mysql://me@db:3306/app?ssl-mode={mode}");
            let options: MySqlConnectOptions = url.parse().unwrap();
            assert_eq!(
                format!("{:?}", options.get_ssl_mode()),
                format!("{expected:?}"),
                "{mode}"
            );
        }
        assert!(
            "mysql://me@db/app?ssl-mode=SOMETIMES"
                .parse::<MySqlConnectOptions>()
                .is_err()
        );
    }
}
