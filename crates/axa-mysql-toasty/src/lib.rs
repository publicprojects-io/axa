//! Toasty connections, migrations, and test resets for Axum services, on MySQL.
//!
//! This crate has the same shape as `axa-postgres-toasty` and the other Axa database
//! crates: a [`Pool`], a [`PoolConfig`], [`connect`], [`ping`], [`migrate`],
//! [`pending`], [`truncate`], [`dangerously_recreate`], [`is_unique_violation`], and
//! [`quote_identifier`]. `axa`'s `mysql-toasty` feature re-exports it as
//! `axa::mysql_toasty`, and makes [`Pool`] the service's `AppContext::db` when no other
//! database feature is on.
//!
//! ```no_run
//! # async fn example() -> toasty::Result<()> {
//! #[derive(Debug, toasty::Model)]
//! struct User {
//!     #[key]
//!     #[auto]
//!     id: u64,
//!     name: String,
//! }
//!
//! let config = axa_mysql_toasty::PoolConfig::new("mysql://app:secret@localhost:3306/app");
//! let db = axa_mysql_toasty::connect(&config, toasty::models!(User)).await?;
//! axa_mysql_toasty::ping(&db).await?;
//! # Ok(())
//! # }
//! ```
//!
//! What differs from Postgres:
//!
//! - URLs are `mysql://user:password@host:port/database`, and must name a database.
//!   Toasty's MySQL driver reads them with `sqlx`, so TLS is asked for with `sqlx`'s
//!   `ssl-mode` query parameter (`DISABLED`, `PREFERRED`, `REQUIRED`, `VERIFY_CA`, or
//!   `VERIFY_IDENTITY`), as with every Axa MySQL crate.
//! - [`quote_identifier`] uses backticks.
//! - [`truncate`] empties each table on one connection with foreign key checks off,
//!   then turns them back on. `TRUNCATE` restarts each table's `AUTO_INCREMENT`.
//! - [`dangerously_recreate`] drops every table in `DATABASE()`, with foreign key
//!   checks off, then migrates.
//! - MySQL commits each DDL statement as it goes, so a migration that fails part way
//!   leaves the statements before the failure applied, though Toasty does not record
//!   the migration as applied.
//! - [`error_number`] and [`sql_state`] read the MySQL error number and `SQLSTATE`
//!   from the `sqlx` error that Toasty's driver wraps. Toasty's driver turns a deadlock
//!   (1213) and a write in a read-only transaction (1792) into Toasty's own kinds of
//!   error, keeping only the message, so for those two neither is known.
//!
//! Toasty migrations go forward only: a [`MigrationSet`] has no down steps, so there
//! is no `revert`. Generate migrations with `toasty-cli`, embed them with
//! [`toasty::embed_migrations!`], and apply them with [`migrate`].

use std::{borrow::Cow, future::Future, pin::Pin, sync::Arc, time::Duration};

use sqlx::mysql::MySqlDatabaseError;
pub use toasty;
use toasty::{
    Db, ModelSet,
    db::{Capability, ConnectContext, Driver},
    migration::MigrationSet,
};
use toasty_core::{
    driver::{
        Connection, ExecResponse, Operation,
        operation::{RawSql, RawSqlRet},
    },
    schema::{
        Schema,
        db::{AppliedMigration, Migration},
        diff,
    },
    stmt::{Type, Value},
};

/// The handle a service holds: Toasty's [`Db`], which owns a connection pool.
pub type Pool = Db;

/// What this crate's functions return on failure.
pub type Error = toasty::Error;

/// The pool's settings. The URL is left out of its `Debug` text, as it may hold a
/// password.
#[derive(Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// The connection URL, such as `mysql://user:password@host:3306/database`.
    pub uri: String,
    /// The most connections the pool opens.
    pub max_connections: u32,
    /// How long to wait to open a connection, or for a free one, in milliseconds.
    pub connect_timeout_ms: u64,
    /// How long a connection may sit unused before it is closed, in milliseconds.
    pub idle_timeout_ms: u64,
    /// `MySQL`'s `max_execution_time` on every connection, in milliseconds, which
    /// stops a read-only `SELECT` that runs longer, and no other statement. `0` sets
    /// none.
    pub statement_timeout_ms: u64,
    /// Make every transaction read-only, with `transaction_read_only`, as for a read
    /// replica or a reporting service, so that a write is refused by the database.
    pub read_only: bool,
}

impl std::fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("max_connections", &self.max_connections)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("statement_timeout_ms", &self.statement_timeout_ms)
            .field("read_only", &self.read_only)
            .finish_non_exhaustive()
    }
}

impl PoolConfig {
    /// Settings for `uri`, with the same defaults as the other Axa database crates:
    /// ten connections, a five-second wait, and ten minutes idle.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            max_connections: 10,
            connect_timeout_ms: 5_000,
            idle_timeout_ms: 600_000,
            statement_timeout_ms: 0,
            read_only: false,
        }
    }

    /// The one statement that sets the session settings on a new connection, or
    /// `None` when there are none to set.
    #[must_use]
    pub fn session_statement(&self) -> Option<String> {
        session_statement(self.statement_timeout_ms, self.read_only)
    }

    /// A Toasty builder with these settings and `models`, ready to connect.
    #[must_use]
    pub fn builder(&self, models: ModelSet) -> toasty::db::Builder {
        let wait = Some(Duration::from_millis(self.connect_timeout_ms));
        let mut builder = Db::builder();
        builder
            .models(models)
            .max_pool_size(usize::try_from(self.max_connections).unwrap_or(usize::MAX))
            .pool_wait_timeout(wait)
            .pool_create_timeout(wait)
            .pool_max_connection_idle_time(Some(Duration::from_millis(self.idle_timeout_ms)));
        builder
    }
}

/// Connect to the database for `models`, and check that it answers.
///
/// # Errors
///
/// Fails when the URL is not one Toasty reads, or the database cannot be reached.
pub async fn connect(config: &PoolConfig, models: ModelSet) -> Result<Pool, Error> {
    let builder = &mut config.builder(models);
    let db = match config.session_statement() {
        Some(setup) => {
            let driver = toasty::db::Connect::new(&config.uri).await?;
            builder
                .build(Setting {
                    driver,
                    setup: setup.into(),
                })
                .await?
        }
        None => builder.connect(&config.uri).await?,
    };
    ping(&db).await?;
    Ok(db)
}

/// `SET SESSION` for each session setting asked for, as one statement, or `None`.
/// The values are a number and a fixed word, so nothing a caller sends reaches it.
fn session_statement(statement_timeout_ms: u64, read_only: bool) -> Option<String> {
    let mut settings = Vec::new();
    if statement_timeout_ms > 0 {
        settings.push(format!(
            "SESSION max_execution_time = {statement_timeout_ms}"
        ));
    }
    if read_only {
        settings.push("SESSION transaction_read_only = ON".to_string());
    }
    (!settings.is_empty()).then(|| format!("SET {}", settings.join(", ")))
}

/// A future a driver returns, as `async_trait` writes it.
type Boxed<'a, T> = Pin<Box<dyn Future<Output = toasty::Result<T>> + Send + 'a>>;

/// Toasty's driver, with the session settings set on each connection before its
/// first operation. Toasty's `MySQL` driver has no hook of its own for it.
#[derive(Debug)]
struct Setting {
    driver: toasty::db::Connect,
    setup: Arc<str>,
}

/// A connection, and the settings it has still to set, if any.
#[derive(Debug)]
struct SettingConnection {
    inner: Box<dyn Connection>,
    setup: Option<Arc<str>>,
}

impl Driver for Setting {
    fn url(&self) -> Cow<'_, str> {
        self.driver.url()
    }

    fn capability(&self) -> &'static Capability {
        self.driver.capability()
    }

    fn connect<'a, 'b, 'c>(&'a self, cx: &'b ConnectContext) -> Boxed<'c, Box<dyn Connection>>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            let inner = self.driver.connect(cx).await?;
            let connection: Box<dyn Connection> = Box::new(SettingConnection {
                inner,
                setup: Some(self.setup.clone()),
            });
            Ok(connection)
        })
    }

    fn max_connections(&self) -> Option<usize> {
        self.driver.max_connections()
    }

    fn generate_migration(&self, schema_diff: &diff::Schema<'_>) -> Migration {
        self.driver.generate_migration(schema_diff)
    }

    fn reset_db<'a, 'c>(&'a self) -> Boxed<'c, ()>
    where
        'a: 'c,
        Self: 'c,
    {
        self.driver.reset_db()
    }
}

impl Connection for SettingConnection {
    fn exec<'a, 'b, 'c>(
        &'a mut self,
        schema: &'b Arc<Schema>,
        plan: Operation,
    ) -> Boxed<'c, ExecResponse>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        Box::pin(async move {
            // Kept until it succeeds, so a connection never runs a query without it.
            if let Some(setup) = &self.setup {
                let set = RawSql {
                    sql: setup.to_string(),
                    params: Vec::new(),
                    ret: RawSqlRet::None,
                };
                self.inner.exec(schema, set.into()).await?;
                self.setup = None;
            }
            self.inner.exec(schema, plan).await
        })
    }

    fn is_valid(&self) -> bool {
        self.inner.is_valid()
    }

    fn ping<'a, 'c>(&'a mut self) -> Boxed<'c, ()>
    where
        'a: 'c,
        Self: 'c,
    {
        self.inner.ping()
    }

    fn push_schema<'a, 'b, 'c>(&'a mut self, schema: &'b Schema) -> Boxed<'c, ()>
    where
        'a: 'c,
        'b: 'c,
        Self: 'c,
    {
        self.inner.push_schema(schema)
    }

    fn applied_migrations<'a, 'c>(&'a mut self) -> Boxed<'c, Vec<AppliedMigration>>
    where
        'a: 'c,
        Self: 'c,
    {
        self.inner.applied_migrations()
    }

    fn apply_migration<'a, 'b, 'm, 'c>(
        &'a mut self,
        id: u64,
        name: &'b str,
        migration: &'m Migration,
    ) -> Boxed<'c, ()>
    where
        'a: 'c,
        'b: 'c,
        'm: 'c,
        Self: 'c,
    {
        self.inner.apply_migration(id, name, migration)
    }
}

/// Check the database connection.
///
/// # Errors
///
/// Fails when the database does not answer.
pub async fn ping(pool: &Pool) -> Result<(), Error> {
    let mut db = pool.clone();
    toasty::sql::query("SELECT 1").exec(&mut db).await?;
    Ok(())
}

/// Apply every migration in `migrations` not yet applied, in order, returning how
/// many were applied.
///
/// # Errors
///
/// Fails on a database error, and then no later migration is tried.
pub async fn migrate(pool: &Pool, migrations: &MigrationSet) -> Result<usize, Error> {
    Ok(migrations.apply(pool).await?.applied())
}

/// The ids Toasty has recorded as applied, from its `__toasty_migrations` table.
async fn applied_ids(pool: &Pool) -> Result<Vec<u64>, Error> {
    let mut db = pool.clone();
    let found = toasty::sql::query(
        "SELECT CAST(COUNT(*) AS SIGNED) FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = '__toasty_migrations'",
    )
    .column_types([Type::I64])
    .exec(&mut db)
    .await?;
    let found = match first_field(&found) {
        Some(Value::I64(count)) => *count,
        _ => 0,
    };
    if found == 0 {
        return Ok(Vec::new());
    }
    let rows = toasty::sql::query("SELECT id FROM __toasty_migrations")
        .column_types([Type::U64])
        .exec(&mut db)
        .await?;
    Ok(rows
        .iter()
        .filter_map(|row| match first_field(std::slice::from_ref(row)) {
            Some(Value::U64(id)) => Some(*id),
            _ => None,
        })
        .collect())
}

/// The first column of the first row.
fn first_field(rows: &[Value]) -> Option<&Value> {
    match rows.first()? {
        Value::Record(record) => record.fields.first(),
        _ => None,
    }
}

/// The names of the migrations in `migrations` not yet applied, in order.
///
/// # Errors
///
/// Fails on a database error.
pub async fn pending(pool: &Pool, migrations: &MigrationSet) -> Result<Vec<String>, Error> {
    let applied = applied_ids(pool).await?;
    Ok(migrations
        .migrations()
        .iter()
        .filter(|migration| !applied.contains(&migration.id()))
        .map(|migration| migration.name().to_string())
        .collect())
}

/// The MySQL error the database answered with, when `error` came from it. Toasty
/// names only a few kinds of database error itself, so this is read from the `sqlx`
/// error that Toasty's MySQL driver wraps.
fn mysql_error(error: &Error) -> Option<&MySqlDatabaseError> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(sqlx) = current.downcast_ref::<sqlx::Error>() {
            return sqlx
                .as_database_error()?
                .try_downcast_ref::<MySqlDatabaseError>();
        }
        if let Some(mysql) = current.downcast_ref::<MySqlDatabaseError>() {
            return Some(mysql);
        }
        source = current.source();
    }
    None
}

/// The MySQL error number the database answered with, such as `1062` for a
/// duplicate entry, when `error` came from the database. `None` for a deadlock
/// (1213) or a write in a read-only transaction (1792), which Toasty's driver turns
/// into its own kinds of error without the number.
#[must_use]
pub fn error_number(error: &Error) -> Option<u16> {
    mysql_error(error).map(MySqlDatabaseError::number)
}

/// The `SQLSTATE` code MySQL answered with, such as `23000` for an integrity
/// constraint violation, when `error` came from the database and MySQL sent one.
/// MySQL's `SQLSTATE` is a broad class, so [`error_number`] says more.
#[must_use]
pub fn sql_state(error: &Error) -> Option<&str> {
    mysql_error(error)?.code()
}

/// Whether `error` is the database refusing a duplicate under a unique constraint:
/// one of MySQL's duplicate-key error numbers (1022, 1062, 1169, 1586, or 1859), as
/// `sqlx` classes them.
#[must_use]
pub fn is_unique_violation(error: &Error) -> bool {
    mysql_error(error).is_some_and(|mysql| {
        sqlx::error::DatabaseError::kind(mysql) == sqlx::error::ErrorKind::UniqueViolation
    })
}

/// `name` in backticks, when it is a plain lower-case identifier: a letter or `_`,
/// then letters, digits, and `_`. Anything else is refused, so it cannot carry SQL.
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("`{name}`"))
}

fn quote_all<S: AsRef<str>>(names: &[S]) -> Result<Vec<String>, Error> {
    names
        .iter()
        .map(|name| {
            quote_identifier(name.as_ref()).ok_or_else(|| {
                toasty::Error::invalid_statement(format!("invalid identifier: {}", name.as_ref()))
            })
        })
        .collect()
}

/// Run `statements` on one connection with foreign key checks off, and turn them
/// back on afterwards, whether or not a statement failed.
async fn without_foreign_key_checks(pool: &Pool, statements: &[String]) -> Result<(), Error> {
    let mut conn = pool.connection().await?;
    toasty::sql::statement("SET FOREIGN_KEY_CHECKS = 0")
        .exec(&mut conn)
        .await?;
    let mut outcome = Ok(());
    for statement in statements {
        if let Err(error) = toasty::sql::statement(statement.as_str())
            .exec(&mut conn)
            .await
        {
            outcome = Err(error);
            break;
        }
    }
    let restored = toasty::sql::statement("SET FOREIGN_KEY_CHECKS = 1")
        .exec(&mut conn)
        .await;
    outcome?;
    restored?;
    Ok(())
}

/// Empty `tables` and restart their `AUTO_INCREMENT`, for a test, with foreign key
/// checks off on one connection, and back on afterwards. Every name must be a plain
/// lower-case identifier.
///
/// # Errors
///
/// Fails on a database error, or a name that is not a plain identifier, before
/// anything is emptied.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let statements: Vec<String> = quote_all(tables)?
        .into_iter()
        .map(|table| format!("TRUNCATE TABLE {table}"))
        .collect();
    without_foreign_key_checks(pool, &statements).await
}

/// Drop every table in the connection's database, `DATABASE()`, Toasty's record of
/// migrations among them, with foreign key checks off, then apply `migrations` from
/// the start. This destroys data: `axa::cli` allows it only in development and
/// test, and only when confirmed.
///
/// # Errors
///
/// Fails on a database error, or a table whose name is not a plain identifier,
/// before anything is dropped.
pub async fn dangerously_recreate(pool: &Pool, migrations: &MigrationSet) -> Result<(), Error> {
    let mut db = pool.clone();
    let rows = toasty::sql::query(
        "SELECT CAST(table_name AS CHAR) FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'",
    )
    .column_types([Type::String])
    .exec(&mut db)
    .await?;
    let tables: Vec<String> = rows
        .iter()
        .filter_map(|row| match first_field(std::slice::from_ref(row)) {
            Some(Value::String(name)) => Some(name.clone()),
            _ => None,
        })
        .collect();
    if !tables.is_empty() {
        let names = quote_all(&tables)?.join(", ");
        without_foreign_key_checks(pool, &[format!("DROP TABLE {names}")]).await?;
    }
    migrate(pool, migrations).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_settings_become_one_set_statement() {
        assert_eq!(session_statement(0, false), None);
        assert_eq!(
            session_statement(5000, true).as_deref(),
            Some("SET SESSION max_execution_time = 5000, SESSION transaction_read_only = ON")
        );
        assert_eq!(
            session_statement(0, true).as_deref(),
            Some("SET SESSION transaction_read_only = ON")
        );
    }

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("mysql://me:hunter2@db:3306/app"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_identifier("users").as_deref(), Some("`users`"));
        assert_eq!(quote_identifier("_a1").as_deref(), Some("`_a1`"));
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a`b", "a\"b", "a;b", "a.b", "users--", "é",
        ] {
            assert_eq!(quote_identifier(name), None, "{name:?}");
        }
        assert!(quote_all(&["good", "bad name"]).is_err());
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        let error = toasty::Error::record_not_found("nothing");
        assert!(!is_unique_violation(&error));
        assert_eq!(error_number(&error), None);
        assert_eq!(sql_state(&error), None);
    }
}
