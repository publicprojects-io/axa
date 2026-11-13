//! `PostgreSQL` pooling, migrations, and test resets over sqlx.
//!
//! This crate holds the operations every service repeats. It does not wrap
//! queries: a service writes its own SQL with `sqlx::query!`, so a reviewer
//! reads exactly what runs.
//!
//! It has the same shape as `axa-postgres-seaorm`, `axa-postgres-diesel`, and `axa-postgres-toasty`: a [`Pool`],
//! a [`PoolConfig`], [`connect`], [`ping`], [`migrate`], [`revert`], [`pending`],
//! [`truncate`], [`dangerously_recreate`], [`is_unique_violation`], and
//! [`quote_identifier`], with [`status`] besides.
//!
//! ```no_run
//! use axa_postgres_sqlx::{PoolConfig, connect, migrate, sqlx};
//!
//! static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate::Migrator::DEFAULT;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = connect(&PoolConfig::new("postgres://localhost/app")).await?;
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
    AssertSqlSafe, PgConnection, PgPool, Postgres, Row,
    migrate::{MigrateError, Migrator},
    pool::PoolConnection,
    postgres::PgPoolOptions,
};

/// The handle a service holds: the pool.
pub type Pool = PgPool;

/// What this crate's connection and query functions return on failure.
pub type Error = sqlx::Error;

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
    pub fn options(&self) -> PgPoolOptions {
        PgPoolOptions::new()
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
/// Fails when the database cannot be reached.
pub async fn connect(config: &PoolConfig) -> Result<PgPool, sqlx::Error> {
    config.options().connect(&config.uri).await
}

/// Check that the database answers.
///
/// # Errors
///
/// Fails when it does not.
pub async fn ping(pool: &PgPool) -> Result<(), sqlx::Error> {
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
/// use axa_postgres_sqlx::{Db, sqlx};
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
    Pool(&'a PgPool),
    /// Use the caller's connection.
    Conn(&'a mut PgConnection),
}

impl<'a> From<&'a PgPool> for Db<'a> {
    fn from(pool: &'a PgPool) -> Self {
        Self::Pool(pool)
    }
}

impl<'a> From<&'a mut PgConnection> for Db<'a> {
    fn from(conn: &'a mut PgConnection) -> Self {
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

/// A connection for the length of one call. It derefs to [`PgConnection`].
pub enum Conn<'a> {
    /// Taken from the pool, and given back when dropped.
    Pooled(PoolConnection<Postgres>),
    /// The caller's.
    Borrowed(&'a mut PgConnection),
}

impl Deref for Conn<'_> {
    type Target = PgConnection;

    fn deref(&self) -> &PgConnection {
        match self {
            Self::Pooled(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

impl DerefMut for Conn<'_> {
    fn deref_mut(&mut self) -> &mut PgConnection {
        match self {
            Self::Pooled(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

/// Whether an error is a violated unique constraint, which a service usually
/// reports as `409` rather than `500`.
#[must_use]
pub fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
}

/// Quote a table or type name for use in SQL.
///
/// Identifiers cannot be bound as parameters, so this is the one place names
/// reach SQL as text. Only lowercase ASCII letters, digits, and underscores
/// are accepted, starting with a letter or underscore. That is stricter than
/// Postgres, and it leaves nothing for an attacker to work with.
///
/// ```
/// assert_eq!(axa_postgres_sqlx::quote_identifier("users").as_deref(), Some("\"users\""));
/// assert_eq!(axa_postgres_sqlx::quote_identifier("users; DROP TABLE x"), None);
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

/// Whether `name` is a custom setting a service may set for row-level security: two or
/// more parts of lower-case ASCII letters, digits, and underscores, joined by dots, such
/// as `app.tenant`. A name with no dot is Postgres's own, such as `role` or
/// `search_path`, which this never sets.
///
/// ```
/// assert!(axa_postgres_sqlx::is_custom_setting("app.tenant"));
/// assert!(!axa_postgres_sqlx::is_custom_setting("search_path"));
/// ```
#[must_use]
pub fn is_custom_setting(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    name.len() <= 63
        && parts.len() >= 2
        && parts.iter().all(|part| {
            part.chars()
                .next()
                .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// Open a transaction with each `(name, value)` set for its length only, such as
/// `("app.tenant", tenant.as_str())` for a row-level security policy that reads
/// `current_setting('app.tenant', true)`.
///
/// Each pair is set with `set_config($1, $2, true)`: bound, never formatted into the
/// SQL, and gone when the transaction commits or rolls back, so a pooled connection
/// never carries one request's tenant into the next. Run every query that needs the
/// setting on the transaction this returns, as a query on the pool may run on another
/// connection.
///
/// ```no_run
/// # async fn example(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
/// let mut tx = axa_postgres_sqlx::begin_with(pool, &[("app.tenant", "acme")]).await?;
/// let count: i64 = sqlx::query_scalar("SELECT count(*) FROM todos")
///     .fetch_one(&mut *tx)
///     .await?;
/// tx.commit().await?;
/// # Ok(())
/// # }
/// ```
///
/// A policy sees nothing unless the role the service connects as is subject to it: not
/// a superuser, without `BYPASSRLS`, and, if it owns the table, with
/// `FORCE ROW LEVEL SECURITY` on it.
///
/// # Errors
///
/// [`sqlx::Error::InvalidArgument`] for a name [`is_custom_setting`] refuses, before
/// anything is sent, and any error from beginning the transaction or setting a value.
pub async fn begin_with(
    pool: &PgPool,
    settings: &[(&str, &str)],
) -> Result<sqlx::Transaction<'static, Postgres>, sqlx::Error> {
    if let Some((name, _)) = settings.iter().find(|(name, _)| !is_custom_setting(name)) {
        return Err(sqlx::Error::InvalidArgument(format!(
            "not a custom setting: {name}"
        )));
    }
    let mut tx = pool.begin().await?;
    for (name, value) in settings {
        sqlx::query("SELECT set_config($1, $2, true)")
            .bind(name)
            .bind(value)
            .execute(&mut *tx)
            .await?;
    }
    Ok(tx)
}

/// Apply pending migrations. Sqlx holds a lock while it does, so services
/// booting together take turns.
///
/// # Errors
///
/// Fails when a migration fails, or an applied one no longer matches.
pub async fn migrate(pool: &PgPool, migrator: &Migrator) -> Result<(), MigrateError> {
    migrator.run(pool).await
}

/// The newest applied versions, newest first. Empty on a database that has
/// never been migrated.
async fn applied_versions(pool: &PgPool) -> Result<Vec<i64>, sqlx::Error> {
    let exists: bool = sqlx::query("SELECT to_regclass('_sqlx_migrations') IS NOT NULL AS found")
        .fetch_one(pool)
        .await?
        .try_get("found")?;
    if !exists {
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
pub async fn revert(pool: &PgPool, migrator: &Migrator, steps: u32) -> Result<(), MigrateError> {
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
    pool: &PgPool,
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
pub async fn pending(pool: &PgPool, migrator: &Migrator) -> Result<Vec<String>, sqlx::Error> {
    Ok(status(pool, migrator)
        .await?
        .into_iter()
        .filter(|migration| !migration.applied)
        .map(|migration| format!("{}_{}", migration.version, migration.description))
        .collect())
}

/// Empty `tables`, restarting their sequences. Each name goes through
/// [`quote_identifier`].
///
/// # Errors
///
/// Fails on an invalid name or a database error.
pub async fn truncate<S: AsRef<str>>(pool: &PgPool, tables: &[S]) -> Result<(), sqlx::Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let tables = quote_all(tables)?.join(", ");
    let sql = format!("TRUNCATE {tables} RESTART IDENTITY CASCADE");
    sqlx::query(AssertSqlSafe(sql)).execute(pool).await?;
    Ok(())
}

/// Drop every table and enum type in `public`, then migrate from nothing.
/// This gives each test an empty schema, whatever tables it touched.
///
/// **This destroys all data.** It exists for tests, and a service must never
/// reach it outside the `test` environment.
///
/// It drops tables and types by name rather than dropping the schema, because
/// `DROP SCHEMA public CASCADE` would also remove the extensions a database
/// was prepared with, such as `citext`. Functions are left in place, so a
/// migration that creates one should use `CREATE OR REPLACE`.
///
/// # Errors
///
/// Fails on a database error, or a name [`quote_identifier`] refuses.
pub async fn dangerously_recreate(pool: &PgPool, migrator: &Migrator) -> Result<(), MigrateError> {
    let tables: Vec<String> =
        sqlx::query("SELECT tablename::text AS name FROM pg_tables WHERE schemaname = 'public'")
            .fetch_all(pool)
            .await?
            .iter()
            .map(|row| row.try_get("name"))
            .collect::<Result<_, _>>()?;
    // Types owned by an extension (`deptype = 'e'`) belong to that extension.
    let types: Vec<String> = sqlx::query(
        "SELECT t.typname::text AS name FROM pg_type t
         JOIN pg_namespace n ON n.oid = t.typnamespace
         WHERE t.typtype = 'e' AND n.nspname = 'public'
           AND NOT EXISTS (
               SELECT 1 FROM pg_depend d WHERE d.objid = t.oid AND d.deptype = 'e'
           )",
    )
    .fetch_all(pool)
    .await?
    .iter()
    .map(|row| row.try_get("name"))
    .collect::<Result<_, _>>()?;

    if !tables.is_empty() {
        let names = quote_all(&tables)?.join(", ");
        let sql = format!("DROP TABLE {names} CASCADE");
        sqlx::query(AssertSqlSafe(sql)).execute(pool).await?;
    }
    for name in quote_all(&types)? {
        let sql = format!("DROP TYPE {name} CASCADE");
        sqlx::query(AssertSqlSafe(sql)).execute(pool).await?;
    }
    migrate(pool, migrator).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("postgres://me:hunter2@db/app"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
    }

    #[test]
    fn only_custom_settings_can_be_set() {
        for name in [
            "app.tenant",
            "app.user_id",
            "my_app.tenant_2",
            "a.b.c",
            "_x.y",
        ] {
            assert!(is_custom_setting(name), "{name}");
        }
        for name in [
            "role",
            "search_path",
            "session_authorization",
            "app.",
            ".tenant",
            "app..tenant",
            "App.Tenant",
            "app.tenant; RESET ROLE",
            "app.2fa",
            "",
        ] {
            assert!(!is_custom_setting(name), "{name}");
        }
        assert!(!is_custom_setting(&format!("app.{}", "a".repeat(60))));
    }

    #[tokio::test]
    async fn a_refused_setting_name_fails_before_anything_is_sent() {
        // This pool would fail to connect, so an error that is not InvalidArgument
        // would mean something was sent.
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap();
        let error = begin_with(&pool, &[("app.tenant", "acme"), ("role", "postgres")])
            .await
            .unwrap_err();
        assert!(
            matches!(&error, sqlx::Error::InvalidArgument(text) if text.contains("role")),
            "{error:?}"
        );
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_identifier("users").as_deref(), Some("\"users\""));
        assert_eq!(quote_identifier("_a1").as_deref(), Some("\"_a1\""));
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a\"b", "a;b", "a.b", "users--", "é",
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
            ..PoolConfig::new("postgres://x")
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
}
