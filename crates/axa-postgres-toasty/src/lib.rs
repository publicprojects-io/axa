//! Toasty connections, migrations, and test resets for Axum services, on
//! PostgreSQL.
//!
//! This crate has the same shape as `axa-postgres-sqlx`, `axa-postgres-seaorm`, and `axa-postgres-diesel`: a
//! [`Pool`], a [`PoolConfig`], [`connect`], [`ping`], [`migrate`], [`pending`],
//! [`truncate`], [`dangerously_recreate`], [`is_unique_violation`], and
//! [`quote_identifier`]. `axa`'s `postgres-toasty` feature re-exports it as `axa::postgres_toasty`, and
//! makes [`Pool`] the service's `AppContext::db` when no other database feature is on.
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
//! let config = axa_postgres_toasty::PoolConfig::new("postgresql://localhost/app");
//! let db = axa_postgres_toasty::connect(&config, toasty::models!(User)).await?;
//! axa_postgres_toasty::ping(&db).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Toasty migrations go forward only: a [`MigrationSet`] has no down steps, so there
//! is no `revert`. Generate migrations with `toasty-cli`, embed them with
//! [`toasty::embed_migrations!`], and apply them with [`migrate`].

use std::time::Duration;

pub use toasty;
use toasty::{Db, ModelSet, Transaction, migration::MigrationSet};
use toasty_core::stmt::{Type, Value};

/// The handle a service holds: Toasty's [`Db`], which owns a connection pool.
pub type Pool = Db;

/// What this crate's functions return on failure.
pub type Error = toasty::Error;

/// The pool's settings. The URL is left out of its `Debug` text, as it may hold a
/// password.
#[derive(Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// The connection URL, such as `postgresql://user:password@host/database`.
    pub uri: String,
    /// The most connections the pool opens.
    pub max_connections: u32,
    /// How long to wait to open a connection, or for a free one, in milliseconds.
    pub connect_timeout_ms: u64,
    /// How long a connection may sit unused before it is closed, in milliseconds.
    pub idle_timeout_ms: u64,
}

impl std::fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("max_connections", &self.max_connections)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .finish_non_exhaustive()
    }
}

impl PoolConfig {
    /// Settings for `uri`, with the same defaults as `axa-postgres-sqlx`: ten connections, a
    /// five-second wait, and ten minutes idle.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            max_connections: 10,
            connect_timeout_ms: 5_000,
            idle_timeout_ms: 600_000,
        }
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
    let db = config.builder(models).connect(&config.uri).await?;
    ping(&db).await?;
    Ok(db)
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
        "SELECT count(*) FROM pg_tables
         WHERE schemaname = current_schema() AND tablename = '__toasty_migrations'",
    )
    .column_types([Type::I64])
    .exec(&mut db)
    .await?;
    if first_i64(&found).unwrap_or(0) == 0 {
        return Ok(Vec::new());
    }
    let rows = toasty::sql::query("SELECT id FROM __toasty_migrations")
        .column_types([Type::I64])
        .exec(&mut db)
        .await?;
    Ok(rows
        .iter()
        .filter_map(|row| first_i64(std::slice::from_ref(row)))
        .filter_map(|id| u64::try_from(id).ok())
        .collect())
}

/// The first column of the first row, as an integer.
fn first_i64(rows: &[Value]) -> Option<i64> {
    match rows.first()? {
        Value::Record(record) => match record.fields.first()? {
            Value::I64(value) => Some(*value),
            _ => None,
        },
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

/// The `SQLSTATE` code Postgres answered with, such as `23505` for a unique
/// violation, when `error` came from the database. Toasty names only a few kinds of
/// database error itself, so the code is read from the driver's error.
#[must_use]
pub fn sql_state(error: &Error) -> Option<&str> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(postgres) = current.downcast_ref::<tokio_postgres::Error>() {
            return postgres.code().map(tokio_postgres::error::SqlState::code);
        }
        source = current.source();
    }
    None
}

/// Whether `error` is the database refusing a duplicate under a unique constraint.
#[must_use]
pub fn is_unique_violation(error: &Error) -> bool {
    sql_state(error) == Some("23505")
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

/// Whether `name` is a custom setting a service may set for row-level security: two or
/// more parts of lower-case ASCII letters, digits, and underscores, joined by dots, such
/// as `app.tenant`. A name with no dot is Postgres's own, such as `role` or
/// `search_path`, which this never sets.
///
/// ```
/// assert!(axa_postgres_toasty::is_custom_setting("app.tenant"));
/// assert!(!axa_postgres_toasty::is_custom_setting("search_path"));
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
/// never carries one request's tenant into the next. A Toasty transaction holds one
/// connection, so run every query that needs the setting on the transaction this
/// returns. Clone the `Db` first if a second handle is needed meanwhile.
///
/// A policy sees nothing unless the role the service connects as is subject to it: not
/// a superuser, without `BYPASSRLS`, and, if it owns the table, with
/// `FORCE ROW LEVEL SECURITY` on it. [`row_security`] checks.
///
/// # Errors
///
/// An invalid-statement error for a name [`is_custom_setting`] refuses, before
/// anything is sent, and any error from beginning the transaction or setting a value.
pub async fn begin_with<'a>(
    db: &'a mut Db,
    settings: &[(&str, &str)],
) -> Result<Transaction<'a>, Error> {
    if let Some((name, _)) = settings.iter().find(|(name, _)| !is_custom_setting(name)) {
        return Err(Error::invalid_statement(format!(
            "not a custom setting: {name}"
        )));
    }
    let mut tx = db.transaction().await?;
    for (name, value) in settings {
        toasty::sql::query("SELECT set_config($1, $2, true)")
            .bind(*name)
            .bind(*value)
            .exec(&mut tx)
            .await?;
    }
    Ok(tx)
}

/// Whether row-level security can keep rows apart for the role a pool connects as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSecurity {
    /// The role the pool connects as.
    pub role: String,
    /// Whether that role skips every policy: a superuser, or a role with `BYPASSRLS`.
    pub bypasses: bool,
    /// The tables asked about that do not exist, have no row-level security, or lack
    /// `FORCE ROW LEVEL SECURITY`, so that their owner skips the policies.
    pub unprotected: Vec<String>,
}

impl RowSecurity {
    /// Whether the policies on every table asked about apply to the role.
    #[must_use]
    pub fn holds(&self) -> bool {
        !self.bypasses && self.unprotected.is_empty()
    }
}

/// The fields of the first row a query returned.
fn first_record(rows: &[Value]) -> Option<&[Value]> {
    match rows.first()? {
        Value::Record(record) => Some(&record.fields),
        _ => None,
    }
}

/// Ask Postgres whether row-level security can apply to `pool`'s role, and whether
/// each of `tables`, such as `todos` or `app.todos`, has it enabled and forced. Call it
/// at boot, after migrating, and refuse to serve when it does not hold.
///
/// # Errors
///
/// Any error from the queries.
pub async fn row_security<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<RowSecurity, Error> {
    let mut db = pool.clone();
    let rows = toasty::sql::query(
        "SELECT current_user::text AS role, rolsuper OR rolbypassrls AS bypasses
         FROM pg_roles WHERE rolname = current_user",
    )
    .column_types([Type::String, Type::Bool])
    .exec(&mut db)
    .await?;
    let (role, bypasses) = match first_record(&rows) {
        Some([Value::String(role), Value::Bool(bypasses)]) => (role.clone(), *bypasses),
        _ => return Err(Error::invalid_result("no row for the current role")),
    };
    let mut unprotected = Vec::new();
    for table in tables {
        let table = table.as_ref();
        let rows = toasty::sql::query(
            "SELECT relrowsecurity AND relforcerowsecurity AS forced FROM pg_class
             WHERE oid = to_regclass($1)",
        )
        .bind(table)
        .column_types([Type::Bool])
        .exec(&mut db)
        .await?;
        if !matches!(first_record(&rows), Some([Value::Bool(true)])) {
            unprotected.push(table.to_string());
        }
    }
    Ok(RowSecurity {
        role,
        bypasses,
        unprotected,
    })
}

/// Empty `tables` and restart their identities, for a test. Every name must be a
/// plain lower-case identifier.
///
/// # Errors
///
/// Fails on a database error, or a name that is not a plain identifier, before
/// anything is emptied.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let tables = quote_all(tables)?.join(", ");
    let mut db = pool.clone();
    toasty::sql::statement(format!("TRUNCATE {tables} RESTART IDENTITY CASCADE"))
        .exec(&mut db)
        .await?;
    Ok(())
}

/// Drop every table in the `public` schema, Toasty's record of migrations among
/// them, then apply `migrations` from the start. This destroys data: `axa::cli`
/// allows it only in development and test, and only when confirmed.
///
/// # Errors
///
/// Fails on a database error.
pub async fn dangerously_recreate(pool: &Pool, migrations: &MigrationSet) -> Result<(), Error> {
    let mut db = pool.clone();
    let rows =
        toasty::sql::query("SELECT tablename::text FROM pg_tables WHERE schemaname = 'public'")
            .column_types([Type::String])
            .exec(&mut db)
            .await?;
    let tables: Vec<String> = rows
        .iter()
        .filter_map(|row| match row {
            Value::Record(record) => match record.fields.first() {
                Some(Value::String(name)) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    if !tables.is_empty() {
        let names = quote_all(&tables)?.join(", ");
        toasty::sql::statement(format!("DROP TABLE {names} CASCADE"))
            .exec(&mut db)
            .await?;
    }
    migrate(pool, migrations).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("postgresql://me:hunter2@db/app"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
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
        assert!(quote_all(&["good", "bad name"]).is_err());
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&toasty::Error::record_not_found(
            "nothing"
        )));
    }
}
