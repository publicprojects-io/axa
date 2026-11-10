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
use toasty::{Db, ModelSet, migration::MigrationSet};
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
