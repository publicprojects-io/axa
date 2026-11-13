//! Database connections from configuration, for whichever database feature is on.
//!
//! Each database crate has the same shape: a `Pool`, a `PoolConfig`, `connect`,
//! `ping`, `migrate`, `pending`, `truncate`, `dangerously_recreate`,
//! `is_unique_violation`, and `quote_identifier`, and `revert` where its migrations
//! have down steps. Here, each `PoolConfig` is built from [`Database`], and
//! [`connect`] opens the handle [`crate::AppContext`] holds.

#[cfg(all(feature = "mysql-seaorm", not(feature = "postgres-seaorm")))]
pub use axa_mysql_seaorm::truncate_table;
#[cfg(feature = "postgres-seaorm")]
pub use axa_postgres_seaorm::truncate_table;
#[cfg(all(
    feature = "sqlite-seaorm",
    not(any(feature = "postgres-seaorm", feature = "mysql-seaorm"))
))]
pub use axa_sqlite_seaorm::truncate_table;

#[cfg(not(axa_db = "none"))]
use crate::Result;
#[cfg(any(
    feature = "postgres-sqlx",
    feature = "postgres-seaorm",
    feature = "postgres-diesel",
    feature = "postgres-toasty",
    feature = "mysql-sqlx",
    feature = "mysql-seaorm",
    feature = "mysql-diesel",
    feature = "mysql-toasty",
    feature = "sqlite-sqlx",
    feature = "sqlite-seaorm",
    feature = "sqlite-diesel"
))]
use crate::config::Database;

/// The pool settings in the configuration.
#[cfg(feature = "postgres-sqlx")]
impl From<&Database> for axa_postgres_sqlx::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
        }
    }
}

/// The pool settings in the configuration.
#[cfg(feature = "postgres-seaorm")]
impl From<&Database> for axa_postgres_seaorm::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
            enable_logging: config.enable_logging,
        }
    }
}

/// The pool settings in the configuration. Diesel's pool keeps no minimum, and
/// closes no idle connection, so it reads the most connections and the wait.
#[cfg(feature = "postgres-diesel")]
impl From<&Database> for axa_postgres_diesel::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            max_connections: usize::try_from(config.max_connections).unwrap_or(usize::MAX),
            wait_timeout: std::time::Duration::from_millis(config.connect_timeout),
        }
    }
}

/// The pool settings in the configuration. Toasty's pool keeps no minimum.
#[cfg(feature = "postgres-toasty")]
impl From<&Database> for axa_postgres_toasty::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
        }
    }
}

/// The pool settings in the configuration.
#[cfg(feature = "mysql-sqlx")]
impl From<&Database> for axa_mysql_sqlx::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
        }
    }
}

/// The pool settings in the configuration.
#[cfg(feature = "mysql-seaorm")]
impl From<&Database> for axa_mysql_seaorm::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
            enable_logging: config.enable_logging,
        }
    }
}

/// The pool settings in the configuration. Diesel's pool keeps no minimum, and
/// closes no idle connection, so it reads the most connections and the wait.
#[cfg(feature = "mysql-diesel")]
impl From<&Database> for axa_mysql_diesel::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            max_connections: usize::try_from(config.max_connections).unwrap_or(usize::MAX),
            wait_timeout: std::time::Duration::from_millis(config.connect_timeout),
        }
    }
}

/// The pool settings in the configuration. Toasty's pool keeps no minimum.
#[cfg(feature = "mysql-toasty")]
impl From<&Database> for axa_mysql_toasty::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
        }
    }
}

/// The pool settings in the configuration.
#[cfg(feature = "sqlite-sqlx")]
impl From<&Database> for axa_sqlite_sqlx::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
            ..axa_sqlite_sqlx::PoolConfig::new(String::new())
        }
    }
}

/// The pool settings in the configuration.
#[cfg(feature = "sqlite-seaorm")]
impl From<&Database> for axa_sqlite_seaorm::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            min_connections: config.min_connections,
            max_connections: config.max_connections,
            connect_timeout_ms: config.connect_timeout,
            idle_timeout_ms: config.idle_timeout,
            enable_logging: config.enable_logging,
            ..axa_sqlite_seaorm::PoolConfig::new(String::new())
        }
    }
}

/// The pool settings in the configuration. Diesel's pool keeps no minimum, and
/// closes no idle connection, so it reads the most connections and the wait.
#[cfg(feature = "sqlite-diesel")]
impl From<&Database> for axa_sqlite_diesel::PoolConfig {
    fn from(config: &Database) -> Self {
        Self {
            uri: config.uri.clone(),
            max_connections: usize::try_from(config.max_connections).unwrap_or(usize::MAX),
            wait_timeout: std::time::Duration::from_millis(config.connect_timeout),
            ..axa_sqlite_diesel::PoolConfig::new(String::new())
        }
    }
}

/// Open the handle [`crate::AppContext`] holds, from configuration.
///
/// # Errors
///
/// Fails when the database cannot be reached.
#[cfg(any(
    axa_db = "postgres_sqlx",
    axa_db = "postgres_seaorm",
    axa_db = "postgres_diesel",
    axa_db = "mysql_sqlx",
    axa_db = "mysql_seaorm",
    axa_db = "mysql_diesel",
    axa_db = "sqlite_sqlx",
    axa_db = "sqlite_seaorm",
    axa_db = "sqlite_diesel"
))]
pub async fn connect(config: &Database) -> Result<crate::context::Db> {
    #[cfg(axa_db = "postgres_sqlx")]
    let db = axa_postgres_sqlx::connect(&config.into()).await?;
    #[cfg(axa_db = "postgres_seaorm")]
    let db = axa_postgres_seaorm::connect(&config.into()).await?;
    #[cfg(axa_db = "postgres_diesel")]
    let db = axa_postgres_diesel::connect(&config.into()).await?;
    #[cfg(axa_db = "mysql_sqlx")]
    let db = axa_mysql_sqlx::connect(&config.into()).await?;
    #[cfg(axa_db = "mysql_seaorm")]
    let db = axa_mysql_seaorm::connect(&config.into()).await?;
    #[cfg(axa_db = "mysql_diesel")]
    let db = axa_mysql_diesel::connect(&config.into()).await?;
    #[cfg(axa_db = "sqlite_sqlx")]
    let db = axa_sqlite_sqlx::connect(&config.into()).await?;
    #[cfg(axa_db = "sqlite_seaorm")]
    let db = axa_sqlite_seaorm::connect(&config.into()).await?;
    #[cfg(axa_db = "sqlite_diesel")]
    let db = axa_sqlite_diesel::connect(&config.into()).await?;
    Ok(db)
}

/// Open the handle [`crate::AppContext`] holds, from configuration, for the models
/// Toasty is to know, such as `toasty::models!(User, Post)`.
///
/// # Errors
///
/// Fails when the database cannot be reached.
#[cfg(any(axa_db = "postgres_toasty", axa_db = "mysql_toasty"))]
pub async fn connect(
    config: &Database,
    models: toasty_models::ModelSet,
) -> Result<crate::context::Db> {
    #[cfg(axa_db = "postgres_toasty")]
    let db = axa_postgres_toasty::connect(&config.into(), models).await?;
    #[cfg(axa_db = "mysql_toasty")]
    let db = axa_mysql_toasty::connect(&config.into(), models).await?;
    Ok(db)
}

#[cfg(axa_db = "mysql_toasty")]
use axa_mysql_toasty::toasty as toasty_models;
/// The `toasty` crate, from whichever Toasty crate is on.
#[cfg(axa_db = "postgres_toasty")]
use axa_postgres_toasty::toasty as toasty_models;

/// Refuse to serve where Postgres row-level security cannot keep tenants apart: the
/// pool's role is a superuser or has `BYPASSRLS`, or one of `tables` lacks row-level
/// security or `FORCE ROW LEVEL SECURITY`. Call it at boot, after migrating, naming
/// each table a policy guards.
///
/// A production-like environment fails, so a misconfigured deployment never serves
/// every tenant's rows. Development and test warn and go on, so a local superuser still
/// works, but a test that proves the policies has to connect as a role they apply to.
///
/// ```no_run
/// # async fn example(pool: &sqlx::PgPool, environment: &axa::Environment) -> axa::Result<()> {
/// axa::db::require_row_security(pool, &["todos"], environment).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// In a production-like environment, when row-level security does not hold, naming the
/// role and the tables. In any environment, when the database cannot be asked.
#[cfg(axa_db = "postgres_sqlx")]
pub async fn require_row_security<S: AsRef<str>>(
    pool: &axa_postgres_sqlx::Pool,
    tables: &[S],
    environment: &crate::Environment,
) -> Result<()> {
    let found = axa_postgres_sqlx::row_security(pool, tables).await?;
    if found.holds() {
        return Ok(());
    }
    let problem = row_security_problem(&found);
    if environment.is_production_like() {
        return Err(crate::Error::string(&problem));
    }
    tracing::warn!(problem = %problem, "row_security_does_not_hold");
    Ok(())
}

/// What is wrong, for an operator: the role and the table names, which come from the
/// database and the service's own code, and nothing a caller sent.
#[cfg(axa_db = "postgres_sqlx")]
fn row_security_problem(found: &axa_postgres_sqlx::RowSecurity) -> String {
    let mut parts = Vec::new();
    if found.bypasses {
        parts.push(format!(
            "role {} is a superuser or has BYPASSRLS, so it skips every policy",
            found.role
        ));
    }
    if !found.unprotected.is_empty() {
        parts.push(format!(
            "these tables are missing, or lack ROW LEVEL SECURITY or FORCE ROW LEVEL SECURITY: {}",
            found.unprotected.join(", ")
        ));
    }
    format!("row-level security does not hold: {}", parts.join("; "))
}

#[cfg(all(test, feature = "postgres-sqlx"))]
mod debug_tests {
    #[tokio::test]
    async fn a_pools_debug_text_holds_no_password() {
        let pool = sqlx::PgPool::connect_lazy("postgres://me:hunter2@db.internal/app").unwrap();
        let printed = format!("{pool:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
    }

    #[test]
    fn the_row_security_problem_names_the_role_and_each_table() {
        let found = axa_postgres_sqlx::RowSecurity {
            role: "app".to_string(),
            bypasses: true,
            unprotected: vec!["todos".to_string(), "notes".to_string()],
        };
        let text = super::row_security_problem(&found);
        assert!(
            text.contains("role app is a superuser or has BYPASSRLS"),
            "{text}"
        );
        assert!(text.contains("todos, notes"), "{text}");
        let only_tables = axa_postgres_sqlx::RowSecurity {
            bypasses: false,
            ..found
        };
        let text = super::row_security_problem(&only_tables);
        assert!(!text.contains("BYPASSRLS,"), "{text}");
        assert!(!text.contains("role app"), "{text}");
    }
}
