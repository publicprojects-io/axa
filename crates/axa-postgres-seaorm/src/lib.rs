//! `SeaORM` connections, migrations, schema helpers, and test resets.
//!
//! This crate has the same shape as `axa-postgres-sqlx`, `axa-postgres-diesel`, and `axa-postgres-toasty`: a
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
//! use axa_postgres_seaorm::{PoolConfig, connect};
//!
//! # async fn run() -> Result<(), axa_postgres_seaorm::sea_orm::DbErr> {
//! let db = connect(&PoolConfig::new("postgres://localhost/app")).await?;
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

pub use sea_orm;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend,
    DbErr, EntityTrait, SqlErr, Statement, TransactionTrait,
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
    /// Postgres's `statement_timeout` on every connection, in milliseconds. `0` sets
    /// none.
    pub statement_timeout_ms: u64,
    /// Postgres's `application_name` on every connection, so `pg_stat_activity` says
    /// which service a query came from. Empty sets none.
    pub application_name: String,
    /// Open every transaction read-only, as for a read replica or a reporting
    /// service, so that a write is refused by the database.
    pub read_only: bool,
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
            .field("statement_timeout_ms", &self.statement_timeout_ms)
            .field("application_name", &self.application_name)
            .field("read_only", &self.read_only)
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
            statement_timeout_ms: 0,
            application_name: String::new(),
            read_only: false,
        }
    }

    /// The `SeaORM` options these settings describe.
    #[must_use]
    pub fn options(&self) -> ConnectOptions {
        self.options_for(self.uri.clone())
    }

    /// These settings, connecting to `uri`.
    fn options_for(&self, uri: String) -> ConnectOptions {
        let mut options = ConnectOptions::new(uri);
        options
            .min_connections(self.min_connections)
            .max_connections(self.max_connections)
            .connect_timeout(Duration::from_millis(self.connect_timeout_ms))
            .idle_timeout(Duration::from_millis(self.idle_timeout_ms))
            .sqlx_logging(self.enable_logging);
        options
    }
}

impl PoolConfig {
    /// The connection string with the session settings added, as every connection
    /// [`connect`] opens uses.
    ///
    /// # Errors
    ///
    /// When the URL sets `options` already and a statement timeout or read-only mode
    /// is set here too, or when the application name is not one Postgres keeps as it
    /// is.
    pub fn connection_uri(&self) -> Result<String, DbErr> {
        session_uri(
            &self.uri,
            self.statement_timeout_ms,
            &self.application_name,
            self.read_only,
        )
        .map_err(DbErr::Custom)
    }
}

/// `uri` with each session setting added as a libpq connection parameter, so that
/// every connection the pool opens starts with them, at no extra round trip:
/// `statement_timeout` and `default_transaction_read_only` in `options`, and
/// `application_name`.
///
/// An `application_name` the URL already names is kept. `options` the URL already
/// names, with a setting here to add, is refused, rather than either one silently
/// replacing the other.
fn session_uri(
    uri: &str,
    statement_timeout_ms: u64,
    application_name: &str,
    read_only: bool,
) -> Result<String, String> {
    let mut options = Vec::new();
    if statement_timeout_ms > 0 {
        options.push(format!("-c statement_timeout={statement_timeout_ms}"));
    }
    if read_only {
        options.push("-c default_transaction_read_only=on".to_string());
    }
    let mut params = Vec::new();
    if !options.is_empty() {
        if has_param(uri, "options") {
            return Err(
                "the connection URL sets options already: set the statement timeout and \
                 read-only mode there, or here, not both"
                    .to_string(),
            );
        }
        params.push(("options", options.join(" ")));
    }
    if !application_name.is_empty() && !has_param(uri, "application_name") {
        if !is_application_name(application_name) {
            return Err(
                "an application name is ASCII letters, digits, `_`, `.`, and `-`, at most 63"
                    .to_string(),
            );
        }
        params.push(("application_name", application_name.to_string()));
    }
    if params.is_empty() {
        return Ok(uri.to_string());
    }
    let separator = if uri.contains('?') { '&' } else { '?' };
    let query: Vec<String> = params
        .iter()
        .map(|(name, value)| format!("{name}={}", percent_encode(value)))
        .collect();
    Ok(format!("{uri}{separator}{}", query.join("&")))
}

/// Whether the query of `uri` names `name`.
fn has_param(uri: &str, name: &str) -> bool {
    uri.split_once('?').is_some_and(|(_, query)| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(name))
    })
}

/// Whether `name` is an application name Postgres keeps as it is: ASCII letters,
/// digits, `_`, `.`, and `-`, from 1 to 63 bytes.
fn is_application_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// `value` with every byte but the unreserved ones percent-encoded.
fn percent_encode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Open a pool.
///
/// # Errors
///
/// Fails when the database cannot be reached.
pub async fn connect(config: &PoolConfig) -> Result<DatabaseConnection, DbErr> {
    Database::connect(config.options_for(config.connection_uri()?)).await
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

/// Whether `name` is a custom setting a service may set for row-level security: two or
/// more parts of lower-case ASCII letters, digits, and underscores, joined by dots, such
/// as `app.tenant`. A name with no dot is Postgres's own, such as `role` or
/// `search_path`, which this never sets.
///
/// ```
/// assert!(axa_postgres_seaorm::is_custom_setting("app.tenant"));
/// assert!(!axa_postgres_seaorm::is_custom_setting("search_path"));
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
/// setting on the transaction this returns.
///
/// A policy sees nothing unless the role the service connects as is subject to it: not
/// a superuser, without `BYPASSRLS`, and, if it owns the table, with
/// `FORCE ROW LEVEL SECURITY` on it. [`row_security`] checks.
///
/// # Errors
///
/// [`DbErr::Custom`] for a name [`is_custom_setting`] refuses, before anything is
/// sent, and any error from beginning the transaction or setting a value.
pub async fn begin_with(
    pool: &Pool,
    settings: &[(&str, &str)],
) -> Result<DatabaseTransaction, DbErr> {
    if let Some((name, _)) = settings.iter().find(|(name, _)| !is_custom_setting(name)) {
        return Err(DbErr::Custom(format!("not a custom setting: {name}")));
    }
    let tx = pool.begin().await?;
    for (name, value) in settings {
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT set_config($1, $2, true)",
            [(*name).into(), (*value).into()],
        ))
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

/// Ask Postgres whether row-level security can apply to `pool`'s role, and whether
/// each of `tables`, such as `todos` or `app.todos`, has it enabled and forced. Call it
/// at boot, after migrating, and refuse to serve when it does not hold.
///
/// # Errors
///
/// Any error from the queries.
pub async fn row_security<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<RowSecurity, DbErr> {
    let role = pool
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_user::text AS role, rolsuper OR rolbypassrls AS bypasses
             FROM pg_roles WHERE rolname = current_user",
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("no row for the current role".to_string()))?;
    let mut unprotected = Vec::new();
    for table in tables {
        let table = table.as_ref();
        let forced = pool
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT relrowsecurity AND relforcerowsecurity AS forced FROM pg_class
                 WHERE oid = to_regclass($1)",
                [table.into()],
            ))
            .await?
            .map(|row| row.try_get::<bool>("", "forced"))
            .transpose()?;
        if forced != Some(true) {
            unprotected.push(table.to_string());
        }
    }
    Ok(RowSecurity {
        role: role.try_get("", "role")?,
        bypasses: role.try_get("", "bypasses")?,
        unprotected,
    })
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
    fn session_settings_become_connection_parameters() {
        let base = "postgres://app@db/app";
        assert_eq!(session_uri(base, 0, "", false).unwrap(), base);
        assert_eq!(
            session_uri(base, 5000, "orders", true).unwrap(),
            "postgres://app@db/app?options=-c%20statement_timeout%3D5000%20-c%20\
             default_transaction_read_only%3Don&application_name=orders"
        );
        assert_eq!(
            session_uri("postgres://db/app?sslmode=require", 0, "orders", false).unwrap(),
            "postgres://db/app?sslmode=require&application_name=orders"
        );
    }

    #[test]
    fn a_url_that_sets_them_already_is_kept_or_refused() {
        // Its own application name wins.
        let named = "postgres://db/app?application_name=mine";
        assert_eq!(session_uri(named, 0, "orders", false).unwrap(), named);
        // Its own options and a setting here would replace one another, so neither does.
        let with_options = "postgres://db/app?options=-c%20work_mem%3D64MB";
        assert!(session_uri(with_options, 5000, "", false).is_err());
        assert!(session_uri(with_options, 0, "", true).is_err());
        assert_eq!(
            session_uri(with_options, 0, "", false).unwrap(),
            with_options
        );
        for name in ["has space", "a&b=c", "é", &"a".repeat(64)] {
            assert!(
                session_uri("postgres://db/app", 0, name, false).is_err(),
                "{name}"
            );
        }
    }

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
