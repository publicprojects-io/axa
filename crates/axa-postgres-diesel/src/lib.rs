//! Diesel pooling, migrations, and test resets, in pure Rust.
//!
//! This uses `diesel-async`, so there is no `libpq` to link and no C in the
//! database path. It holds the operations every service repeats, and a service
//! writes its own queries with Diesel as usual.
//!
//! ```ignore
//! use axa_postgres_diesel::{PoolConfig, connect, migrate};
//! use diesel_migrations::{EmbeddedMigrations, embed_migrations};
//!
//! const MIGRATIONS: EmbeddedMigrations = embed_migrations!();
//!
//! let pool = connect(&PoolConfig::new("postgres://localhost/app")).await?;
//! migrate(&pool, MIGRATIONS).await?;
//! ```
//!
//! # TLS
//!
//! Connections negotiate TLS with `Rustls`, trusting the standard web roots.
//! The connection string's `sslmode` decides how strict that is, and in
//! production it should say `sslmode=require`, because the `prefer` default
//! falls back to plain text when a server offers no TLS.
//!
//! A database behind a private certificate authority needs a custom connection
//! setup, which [`pool_with_tls`] accepts.

use std::{sync::Arc, time::Duration};

use deadpool::Runtime;
pub use diesel;
use diesel::{
    ConnectionError, ConnectionResult, QueryableByName,
    migration::MigrationSource,
    pg::Pg,
    result::{DatabaseErrorKind, Error as QueryError},
    sql_types::{Bool, Text},
};
pub use diesel_async;
use diesel_async::{
    AsyncConnection, AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl, TransactionManager,
    pooled_connection::{
        AsyncDieselConnectionManager, ManagerConfig,
        deadpool::{BuildError, Object, Pool as DeadPool, PoolError},
    },
};
pub use diesel_migrations;
use diesel_migrations::MigrationHarness;
use tokio_postgres_rustls::MakeRustlsConnect;

/// A connection pool.
pub type Pool = DeadPool<AsyncPgConnection>;

/// A pooled connection.
pub type Connection = Object<AsyncPgConnection>;

/// A failure in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The pool could not be built.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// No connection could be had.
    #[error(transparent)]
    Pool(#[from] PoolError),
    /// A query failed.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// A migration failed.
    #[error("migration failed: {0}")]
    Migration(String),
    /// A table or type name was refused by [`quote_identifier`].
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
}

/// Pool settings. The connection string is left out of the `Debug` text, as it may
/// hold a password.
#[derive(Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// Connection string.
    pub uri: String,
    /// Largest number of connections.
    pub max_connections: usize,
    /// How long a caller waits for a connection, whether a free one or a new one,
    /// before it gets an error.
    pub wait_timeout: Duration,
}

impl std::fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("max_connections", &self.max_connections)
            .field("wait_timeout", &self.wait_timeout)
            .finish_non_exhaustive()
    }
}

impl PoolConfig {
    /// Settings for `uri` with a small pool.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            max_connections: 10,
            wait_timeout: Duration::from_secs(5),
        }
    }
}

/// TLS settings trusting the standard web roots, as [`connect`] uses. Pass them to
/// [`pool_with_tls`] to build a pool that connects only when first used.
///
/// # Errors
///
/// Fails when `rustls` refuses the protocol versions, which it does not for its own
/// defaults.
pub fn web_tls() -> Result<rustls::ClientConfig, rustls::Error> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    Ok(rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth())
}

/// Open one connection using `tls`.
async fn establish(uri: String, tls: rustls::ClientConfig) -> ConnectionResult<AsyncPgConnection> {
    let (client, connection) = tokio_postgres::connect(&uri, MakeRustlsConnect::new(tls))
        .await
        .map_err(|error| ConnectionError::BadConnection(error.to_string()))?;
    AsyncPgConnection::try_from_client_and_connection(client, connection).await
}

/// Build a pool using `tls`, without connecting yet.
///
/// # Errors
///
/// Fails when the pool's settings are invalid.
pub fn pool_with_tls(config: &PoolConfig, tls: rustls::ClientConfig) -> Result<Pool, Error> {
    let mut setup = ManagerConfig::default();
    setup.custom_setup = Box::new(move |uri| {
        let uri = uri.to_string();
        let tls = tls.clone();
        Box::pin(establish(uri, tls))
    });
    let manager = AsyncDieselConnectionManager::new_with_config(&config.uri, setup);
    // With no limit, a caller waits for ever behind a pool that is all in use.
    Ok(Pool::builder(manager)
        .max_size(config.max_connections)
        .wait_timeout(Some(config.wait_timeout))
        .create_timeout(Some(config.wait_timeout))
        .runtime(Runtime::Tokio1)
        .build()?)
}

/// Open a pool and prove it works with one connection.
///
/// # Errors
///
/// Fails when the database cannot be reached.
pub async fn connect(config: &PoolConfig) -> Result<Pool, Error> {
    let tls =
        web_tls().map_err(|error| Error::Query(QueryError::QueryBuilderError(Box::new(error))))?;
    let pool = pool_with_tls(config, tls)?;
    ping(&pool).await?;
    Ok(pool)
}

/// Check that the database answers.
///
/// # Errors
///
/// Fails when it does not.
pub async fn ping(pool: &Pool) -> Result<(), Error> {
    let mut connection = pool.get().await?;
    diesel::sql_query("SELECT 1")
        .execute(&mut connection)
        .await?;
    Ok(())
}

/// Whether an error is a violated unique constraint, which a service usually
/// reports as `409` rather than `500`.
#[must_use]
pub fn is_unique_violation(error: &QueryError) -> bool {
    matches!(
        error,
        QueryError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)
    )
}

/// Quote a table or type name for use in SQL.
///
/// Identifiers cannot be bound as parameters, so this is the one place names
/// reach SQL as text. Only lowercase ASCII letters, digits, and underscores
/// are accepted, starting with a letter or underscore.
///
/// ```
/// assert_eq!(axa_postgres_diesel::quote_identifier("users").as_deref(), Some("\"users\""));
/// assert_eq!(axa_postgres_diesel::quote_identifier("users; DROP TABLE x"), None);
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
fn quote_all<S: AsRef<str>>(names: &[S]) -> Result<Vec<String>, Error> {
    names
        .iter()
        .map(|name| {
            quote_identifier(name.as_ref())
                .ok_or_else(|| Error::InvalidIdentifier(name.as_ref().to_string()))
        })
        .collect()
}

/// Run `work` on a migration harness. The harness blocks, so it gets a blocking
/// thread rather than stalling the runtime.
async fn with_harness<T, F>(pool: &Pool, work: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: FnOnce(&mut AsyncMigrationHarness<Connection>) -> Result<T, String> + Send + 'static,
{
    let connection = pool.get().await?;
    tokio::task::spawn_blocking(move || work(&mut AsyncMigrationHarness::new(connection)))
        .await
        .map_err(|error| Error::Migration(error.to_string()))?
        .map_err(Error::Migration)
}

/// Apply pending migrations, returning how many ran.
///
/// # Errors
///
/// Fails when a migration fails.
pub async fn migrate<M>(pool: &Pool, migrations: M) -> Result<usize, Error>
where
    M: MigrationSource<Pg> + Send + 'static,
{
    with_harness(pool, move |harness| {
        harness
            .run_pending_migrations(migrations)
            .map(|applied| applied.len())
            .map_err(|error| error.to_string())
    })
    .await
}

/// Revert the newest `steps` migrations, returning how many were reverted.
///
/// # Errors
///
/// Fails when a down migration fails.
pub async fn revert<M>(pool: &Pool, migrations: M, steps: usize) -> Result<usize, Error>
where
    M: MigrationSource<Pg> + Clone + Send + 'static,
{
    with_harness(pool, move |harness| {
        let mut reverted = 0;
        while reverted < steps {
            let applied = harness
                .applied_migrations()
                .map_err(|error| error.to_string())?;
            if applied.is_empty() {
                break;
            }
            harness
                .revert_last_migration(migrations.clone())
                .map_err(|error| error.to_string())?;
            reverted += 1;
        }
        Ok(reverted)
    })
    .await
}

/// The names of migrations that have not run.
///
/// # Errors
///
/// Fails on a database error.
pub async fn pending<M>(pool: &Pool, migrations: M) -> Result<Vec<String>, Error>
where
    M: MigrationSource<Pg> + Send + 'static,
{
    with_harness(pool, move |harness| {
        let pending = harness
            .pending_migrations(migrations)
            .map_err(|error| error.to_string())?;
        Ok(pending
            .iter()
            .map(|migration| migration.name().to_string())
            .collect())
    })
    .await
}

/// Empty `tables`, restarting their sequences. Each name goes through
/// [`quote_identifier`].
///
/// # Errors
///
/// Fails on an invalid name or a database error.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let tables = quote_all(tables)?.join(", ");
    let mut connection = pool.get().await?;
    diesel::sql_query(format!("TRUNCATE {tables} RESTART IDENTITY CASCADE"))
        .execute(&mut connection)
        .await?;
    Ok(())
}

/// Whether `name` is a custom setting a service may set for row-level security: two or
/// more parts of lower-case ASCII letters, digits, and underscores, joined by dots, such
/// as `app.tenant`. A name with no dot is Postgres's own, such as `role` or
/// `search_path`, which this never sets.
///
/// ```
/// assert!(axa_postgres_diesel::is_custom_setting("app.tenant"));
/// assert!(!axa_postgres_diesel::is_custom_setting("search_path"));
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

/// Set each `(name, value)` for the rest of the open transaction only, such as
/// `("app.tenant", tenant.as_str())` for a row-level security policy that reads
/// `current_setting('app.tenant', true)`. Call it first inside the service's own
/// transaction, so every query after it on `connection` sees the setting:
///
/// ```no_run
/// # use axa_postgres_diesel::diesel_async::{AsyncConnection, AsyncPgConnection};
/// # async fn example(connection: &mut AsyncPgConnection) -> Result<(), diesel::result::Error> {
/// connection
///     .transaction(async |connection| {
///         axa_postgres_diesel::set_local(connection, &[("app.tenant", "acme")]).await?;
///         // The tenant's queries, on `connection`.
///         Ok::<_, diesel::result::Error>(())
///     })
///     .await
/// # }
/// ```
///
/// Each pair is set with `set_config($1, $2, true)`: bound, never formatted into the
/// SQL, and gone when the transaction commits or rolls back, so a pooled connection
/// never carries one request's tenant into the next.
///
/// A policy sees nothing unless the role the service connects as is subject to it: not
/// a superuser, without `BYPASSRLS`, and, if it owns the table, with
/// `FORCE ROW LEVEL SECURITY` on it. [`row_security`] checks.
///
/// # Errors
///
/// [`QueryError::NotInTransaction`] with no transaction open, as the setting would then
/// last for one statement only. [`QueryError::QueryBuilderError`] for a name
/// [`is_custom_setting`] refuses. Both come before anything is sent. Then any error from
/// setting a value.
pub async fn set_local(
    connection: &mut AsyncPgConnection,
    settings: &[(&str, &str)],
) -> Result<(), QueryError> {
    let depth =
        <AsyncPgConnection as AsyncConnection>::TransactionManager::transaction_manager_status_mut(
            connection,
        )
        .transaction_depth()?;
    if depth.is_none() {
        return Err(QueryError::NotInTransaction);
    }
    if let Some((name, _)) = settings.iter().find(|(name, _)| !is_custom_setting(name)) {
        return Err(QueryError::QueryBuilderError(
            format!("not a custom setting: {name}").into(),
        ));
    }
    for (name, value) in settings {
        diesel::sql_query("SELECT set_config($1, $2, true)")
            .bind::<Text, _>(*name)
            .bind::<Text, _>(*value)
            .execute(connection)
            .await?;
    }
    Ok(())
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

/// The current role, and whether it skips every policy.
#[derive(QueryableByName)]
struct Role {
    #[diesel(sql_type = Text)]
    role: String,
    #[diesel(sql_type = Bool)]
    bypasses: bool,
}

/// Whether a table has row-level security enabled and forced.
#[derive(QueryableByName)]
struct Forced {
    #[diesel(sql_type = Bool)]
    forced: bool,
}

/// Ask Postgres whether row-level security can apply to `pool`'s role, and whether
/// each of `tables`, such as `todos` or `app.todos`, has it enabled and forced. Call it
/// at boot, after migrating, and refuse to serve when it does not hold.
///
/// # Errors
///
/// Any error from getting a connection or from the queries.
pub async fn row_security<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<RowSecurity, Error> {
    let mut connection = pool.get().await?;
    let role: Role = diesel::sql_query(
        "SELECT current_user::text AS role, rolsuper OR rolbypassrls AS bypasses
         FROM pg_roles WHERE rolname = current_user",
    )
    .get_result(&mut connection)
    .await?;
    let mut unprotected = Vec::new();
    for table in tables {
        let table = table.as_ref();
        let forced: Vec<Forced> = diesel::sql_query(
            "SELECT relrowsecurity AND relforcerowsecurity AS forced FROM pg_class
             WHERE oid = to_regclass($1)",
        )
        .bind::<Text, _>(table)
        .load(&mut connection)
        .await?;
        if !<[Forced]>::first(&forced).is_some_and(|row| row.forced) {
            unprotected.push(table.to_string());
        }
    }
    Ok(RowSecurity {
        role: role.role,
        bypasses: role.bypasses,
        unprotected,
    })
}

/// One name from a catalog query.
#[derive(QueryableByName)]
struct Name {
    #[diesel(sql_type = Text)]
    name: String,
}

/// Drop every table and enum type in `public`, then migrate from nothing.
/// This gives each test an empty schema, whatever tables it touched.
///
/// **This destroys all data.** It exists for tests, and a service must never
/// reach it outside the `test` environment.
///
/// It drops tables and types by name rather than dropping the schema, which
/// would also remove extensions such as `citext`.
///
/// # Errors
///
/// Fails on a database error, a failed migration, or a name
/// [`quote_identifier`] refuses.
pub async fn dangerously_recreate<M>(pool: &Pool, migrations: M) -> Result<(), Error>
where
    M: MigrationSource<Pg> + Send + 'static,
{
    let mut connection = pool.get().await?;
    let tables: Vec<Name> = diesel::sql_query(
        "SELECT tablename::text AS name FROM pg_tables WHERE schemaname = 'public'",
    )
    .load(&mut connection)
    .await?;
    // Types owned by an extension (`deptype = 'e'`) belong to that extension.
    let types: Vec<Name> = diesel::sql_query(
        "SELECT t.typname::text AS name FROM pg_type t
         JOIN pg_namespace n ON n.oid = t.typnamespace
         WHERE t.typtype = 'e' AND n.nspname = 'public'
           AND NOT EXISTS (
               SELECT 1 FROM pg_depend d WHERE d.objid = t.oid AND d.deptype = 'e'
           )",
    )
    .load(&mut connection)
    .await?;
    let table_names: Vec<&str> = tables.iter().map(|table| table.name.as_str()).collect();
    if !table_names.is_empty() {
        let names = quote_all(&table_names)?.join(", ");
        diesel::sql_query(format!("DROP TABLE {names} CASCADE"))
            .execute(&mut connection)
            .await?;
    }
    let type_names: Vec<&str> = types.iter().map(|name| name.name.as_str()).collect();
    for name in quote_all(&type_names)? {
        diesel::sql_query(format!("DROP TYPE {name} CASCADE"))
            .execute(&mut connection)
            .await?;
    }
    drop(connection);
    migrate(pool, migrations).await?;
    Ok(())
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
    fn identifiers_are_quoted() {
        assert_eq!(quote_identifier("users").as_deref(), Some("\"users\""));
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a\"b", "a;b", "a.b", "users--",
        ] {
            assert_eq!(quote_identifier(name), None, "{name:?}");
        }
    }

    #[test]
    fn quote_all_fails_on_any_bad_name() {
        assert!(quote_all(&["good"]).is_ok());
        assert!(matches!(
            quote_all(&["good", "bad name"]),
            Err(Error::InvalidIdentifier(name)) if name == "bad name"
        ));
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&QueryError::NotFound));
    }

    #[test]
    fn tls_settings_build() {
        assert!(web_tls().is_ok());
    }
}
