//! The state every handler receives.

use std::sync::Arc;

use crate::{Environment, config::Config, health::Checks, store::Store};

/// The database handle [`AppContext`] holds: the `Pool` of the database feature that is
/// on, such as a `PgPool` with `postgres-sqlx` or a Diesel pool with `mysql-diesel`. With
/// more than one on, Postgres wins, then MySQL, then SQLite, and for each, sqlx, then
/// `SeaORM`, Diesel, and Toasty. With none, there is no database.
#[cfg(axa_db = "postgres_sqlx")]
pub type Db = axa_postgres_sqlx::Pool;
/// The database handle [`AppContext`] holds: `axa-postgres-seaorm`'s pool.
#[cfg(axa_db = "postgres_seaorm")]
pub type Db = axa_postgres_seaorm::Pool;
/// The database handle [`AppContext`] holds: `axa-postgres-diesel`'s pool.
#[cfg(axa_db = "postgres_diesel")]
pub type Db = axa_postgres_diesel::Pool;
/// The database handle [`AppContext`] holds: `axa-postgres-toasty`'s pool.
#[cfg(axa_db = "postgres_toasty")]
pub type Db = axa_postgres_toasty::Pool;
/// The database handle [`AppContext`] holds: `axa-mysql-sqlx`'s pool.
#[cfg(axa_db = "mysql_sqlx")]
pub type Db = axa_mysql_sqlx::Pool;
/// The database handle [`AppContext`] holds: `axa-mysql-seaorm`'s pool.
#[cfg(axa_db = "mysql_seaorm")]
pub type Db = axa_mysql_seaorm::Pool;
/// The database handle [`AppContext`] holds: `axa-mysql-diesel`'s pool.
#[cfg(axa_db = "mysql_diesel")]
pub type Db = axa_mysql_diesel::Pool;
/// The database handle [`AppContext`] holds: `axa-mysql-toasty`'s pool.
#[cfg(axa_db = "mysql_toasty")]
pub type Db = axa_mysql_toasty::Pool;
/// The database handle [`AppContext`] holds: `axa-sqlite-sqlx`'s pool.
#[cfg(axa_db = "sqlite_sqlx")]
pub type Db = axa_sqlite_sqlx::Pool;
/// The database handle [`AppContext`] holds: `axa-sqlite-seaorm`'s pool.
#[cfg(axa_db = "sqlite_seaorm")]
pub type Db = axa_sqlite_seaorm::Pool;
/// The database handle [`AppContext`] holds: `axa-sqlite-diesel`'s pool.
#[cfg(axa_db = "sqlite_diesel")]
pub type Db = axa_sqlite_diesel::Pool;
/// The database handle [`AppContext`] holds; none without a database feature.
#[cfg(axa_db = "none")]
pub type Db = ();

/// Check that the database answers, whichever it is.
///
/// # Errors
///
/// Fails when the database does not answer.
#[cfg(not(axa_db = "none"))]
pub async fn ping(db: &Db) -> crate::Result<()> {
    #[cfg(axa_db = "postgres_sqlx")]
    axa_postgres_sqlx::ping(db).await?;
    #[cfg(axa_db = "postgres_seaorm")]
    axa_postgres_seaorm::ping(db).await?;
    #[cfg(axa_db = "postgres_diesel")]
    axa_postgres_diesel::ping(db).await?;
    #[cfg(axa_db = "postgres_toasty")]
    axa_postgres_toasty::ping(db).await?;
    #[cfg(axa_db = "mysql_sqlx")]
    axa_mysql_sqlx::ping(db).await?;
    #[cfg(axa_db = "mysql_seaorm")]
    axa_mysql_seaorm::ping(db).await?;
    #[cfg(axa_db = "mysql_diesel")]
    axa_mysql_diesel::ping(db).await?;
    #[cfg(axa_db = "mysql_toasty")]
    axa_mysql_toasty::ping(db).await?;
    #[cfg(axa_db = "sqlite_sqlx")]
    axa_sqlite_sqlx::ping(db).await?;
    #[cfg(axa_db = "sqlite_seaorm")]
    axa_sqlite_seaorm::ping(db).await?;
    #[cfg(axa_db = "sqlite_diesel")]
    axa_sqlite_diesel::ping(db).await?;
    Ok(())
}

/// A database handle that was never opened, so that a test can build a context, and
/// whose ping fails. A sqlx pool needs a runtime to exist, even one that never connects.
#[cfg(all(
    test,
    any(
        axa_db = "postgres_sqlx",
        axa_db = "mysql_sqlx",
        axa_db = "sqlite_sqlx"
    )
))]
#[allow(clippy::expect_used, reason = "RCA-SCOPE-4: tests may expect")]
fn test_db() -> Db {
    static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
        std::sync::LazyLock::new(|| tokio::runtime::Runtime::new().expect("a runtime"));
    let _entered = RUNTIME.enter();
    let options =
        sqlx::pool::PoolOptions::new().acquire_timeout(std::time::Duration::from_millis(50));
    #[cfg(axa_db = "postgres_sqlx")]
    let uri = "postgres://axa:axa@127.0.0.1:1/axa";
    #[cfg(axa_db = "mysql_sqlx")]
    let uri = "mysql://axa:axa@127.0.0.1:1/axa";
    // A file in a folder that is not there, which nothing creates.
    #[cfg(axa_db = "sqlite_sqlx")]
    let uri = "sqlite:///axa-no-such-folder/never.sqlite3";
    options.connect_lazy(uri).expect("a well-formed address")
}
#[cfg(all(
    test,
    any(
        axa_db = "postgres_seaorm",
        axa_db = "mysql_seaorm",
        axa_db = "sqlite_seaorm"
    )
))]
fn test_db() -> Db {
    sea_orm::DatabaseConnection::default()
}
#[cfg(all(test, axa_db = "postgres_diesel"))]
#[allow(clippy::expect_used, reason = "RCA-SCOPE-4: tests may expect")]
fn test_db() -> Db {
    let config = axa_postgres_diesel::PoolConfig {
        wait_timeout: std::time::Duration::from_millis(50),
        ..axa_postgres_diesel::PoolConfig::new("postgres://axa:axa@127.0.0.1:1/axa")
    };
    let tls = axa_postgres_diesel::web_tls().expect("TLS settings");
    axa_postgres_diesel::pool_with_tls(&config, tls).expect("a pool that never connects")
}
#[cfg(all(test, axa_db = "mysql_diesel"))]
fn test_db() -> Db {
    let config = axa_mysql_diesel::PoolConfig {
        wait_timeout: std::time::Duration::from_millis(50),
        ..axa_mysql_diesel::PoolConfig::new("mysql://axa:axa@127.0.0.1:1/axa")
    };
    axa_mysql_diesel::pool(&config).expect("a pool that never connects")
}
#[cfg(all(test, axa_db = "sqlite_diesel"))]
fn test_db() -> Db {
    let config = axa_sqlite_diesel::PoolConfig {
        wait_timeout: std::time::Duration::from_millis(50),
        ..axa_sqlite_diesel::PoolConfig::new("sqlite:///axa-no-such-folder/never.sqlite3")
    };
    axa_sqlite_diesel::pool(&config).expect("a pool that never connects")
}
#[cfg(all(test, any(axa_db = "postgres_toasty", axa_db = "mysql_toasty")))]
fn test_db() -> Db {
    unimplemented!(
        "Toasty connects when its handle is made, so a unit test with Toasty as the only \
         database feature has no context"
    )
}
#[cfg(all(test, axa_db = "none"))]
fn test_db() -> Db {}

/// Shared service state, used as ordinary Axum state with
/// `State(ctx): State<AppContext>`.
#[derive(Clone)]
pub struct AppContext {
    /// The running environment.
    pub environment: Environment,
    /// The database connection pool.
    pub db: Db,
    /// Service configuration.
    pub config: Arc<Config>,
    /// Values shared across handlers.
    pub shared_store: Arc<Store>,
    /// What `/_health/ready` asks, besides the database and queue that Axa
    /// knows about. Set this once, before building the routes.
    pub health_checks: Checks,
    /// The job queue, when one is configured.
    #[cfg(feature = "queue")]
    pub queue_provider: Option<Arc<crate::queue::Queue>>,
}

/// The database is named by its type, as a Diesel pool has no `Debug` text, and
/// none of them would add anything a reader needs.
impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("AppContext");
        debug
            .field("environment", &self.environment)
            .field("db", &std::any::type_name::<Db>())
            .field("config", &self.config)
            .field("shared_store", &self.shared_store)
            .field("health_checks", &self.health_checks);
        #[cfg(feature = "queue")]
        debug.field("queue_provider", &self.queue_provider);
        debug.finish()
    }
}

impl AppContext {
    /// Build a context with an empty store, no named checks, and no queue.
    #[must_use]
    pub fn new(environment: Environment, db: Db, config: Config) -> Self {
        crate::tls::install_provider();
        Self {
            environment,
            db,
            config: Arc::new(config),
            shared_store: Arc::new(Store::default()),
            health_checks: Checks::new(),
            #[cfg(feature = "queue")]
            queue_provider: None,
        }
    }

    /// A context for a test, whose database was never opened.
    #[cfg(test)]
    #[allow(clippy::unit_arg, dead_code)]
    pub(crate) fn for_test(environment: Environment, config: Config) -> Self {
        Self::new(environment, test_db(), config)
    }
}

/// With the `webhooks` feature, [`axa_webhooks::Verified`] takes its verifier from the
/// context's `shared_store`, where the service puts it with
/// `ctx.shared_store.insert(verifier)`. Without one there, every delivery is refused
/// with a `500`, code `webhook_verifier_missing`, and an error logged, so that a service
/// that forgot its key fails closed and is seen to.
#[cfg(feature = "webhooks")]
impl axum::extract::FromRef<AppContext> for axa_webhooks::Verifier {
    fn from_ref(ctx: &AppContext) -> Self {
        ctx.shared_store
            .get::<Self>()
            .filter(Self::has_keys)
            .unwrap_or_else(|| {
                tracing::error!(
                    "a webhook delivery arrived, but no Verifier with a key is in the \
                     shared store, so it is refused"
                );
                Self::without_keys()
            })
    }
}

#[cfg(all(test, feature = "webhooks"))]
#[allow(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
mod webhook_tests {
    use axa_webhooks::{Signer, SigningKey, Verified, Verifier};
    use axum::{Router, body::Body, http::Request, http::StatusCode, routing::post};
    use tower::ServiceExt;

    use super::AppContext;
    use crate::{
        Environment,
        config::{Config, Defaults},
    };

    fn router(verifier: Option<Verifier>) -> Router {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let ctx = AppContext::for_test(Environment::Test, config);
        if let Some(verifier) = verifier {
            ctx.shared_store.insert(verifier);
        }
        Router::new()
            .route(
                "/hook",
                post(|verified: Verified| async move { verified.delivery.id }),
            )
            .with_state(ctx)
    }

    async fn deliver(router: Router, signer: &Signer) -> (StatusCode, String) {
        let mut request = Request::post("/hook").body(Body::from("{}")).unwrap();
        *request.headers_mut() = signer.headers_now("msg_1", b"{}").unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn verified_takes_its_verifier_from_the_shared_store_and_fails_closed() {
        let key: SigningKey = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".parse().unwrap();
        let signer = Signer::new(key.clone());

        let set = router(Some(Verifier::new(key.verifying_key())));
        assert_eq!(
            deliver(set, &signer).await,
            (StatusCode::OK, "msg_1".into())
        );

        for unset in [router(None), router(Some(Verifier::without_keys()))] {
            let (status, body) = deliver(unset, &signer).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert!(body.contains("webhook_verifier_missing"), "{body}");
        }
    }
}

// Toasty connects when its handle is made, so a Toasty-only build has no context to show.
#[cfg(all(test, not(any(axa_db = "postgres_toasty", axa_db = "mysql_toasty"))))]
#[allow(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
mod tests {
    use super::{AppContext, Db};
    use crate::{
        Environment,
        config::{Config, Defaults},
    };

    #[test]
    fn its_debug_text_names_the_database_by_type_and_shows_the_rest() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let ctx = AppContext::for_test(Environment::Test, config);
        let text = format!("{ctx:?}");
        assert!(text.starts_with("AppContext {"), "{text}");
        assert!(
            text.contains(&format!("db: {:?}", std::any::type_name::<Db>())),
            "{text}"
        );
        for field in [
            "environment: Test",
            "config: ",
            "shared_store: ",
            "health_checks: ",
        ] {
            assert!(text.contains(field), "{field} in {text}");
        }
        #[cfg(feature = "queue")]
        assert!(text.contains("queue_provider: None"), "{text}");
    }
}
