//! Web sessions for Axum services, over [`axum_session`], with safe defaults.
//!
//! `axum_session` does the work. This crate decides what a service gets if it
//! says nothing, because a session that fails open gives no sign of it:
//!
//! | Setting | `axum_session` alone | [`SessionOptions::production`] |
//! |---|---|---|
//! | Cookie signing | Off until a key is set | Required |
//! | Data in the database | Plain text until a key is set | Encrypted, key required |
//! | `Secure` cookie | Off | On |
//! | `SameSite` | `Lax` | `Lax` |
//! | Session lifetime | 6 hours | 8 hours |
//! | Cookie lifetime | 100 days | 7 days |
//! | Bound to the client's IP | Yes, when signed | No: phones change networks |
//!
//! Unsigned cookies are possible only through [`SessionOptions::development`].
//!
//! ```
//! use axa_session::{Session, SessionNullPool, SessionOptions, layer, memory_store};
//! use axum::{Router, routing::get};
//!
//! async fn visits(session: Session<SessionNullPool>) -> String {
//!     let count = session.get::<u32>("count").unwrap_or(0) + 1;
//!     session.set("count", count);
//!     count.to_string()
//! }
//!
//! # async fn run() -> Result<(), axa_session::Error> {
//! let store = memory_store(&SessionOptions::development()).await?;
//! let app: Router = Router::new().route("/", get(visits)).layer(layer(store));
//! # Ok(())
//! # }
//! ```
//!
//! # Keys
//!
//! Give a production service two different keys of at least [`MIN_KEY_BYTES`]
//! bytes, from a secret store. They must stay the same across restarts and
//! replicas: a new signing key signs everyone out.
//!
//! # What a service must still do
//!
//! - **Call [`Session::renew`] when someone signs in or changes privilege.**
//!   Otherwise an identifier planted before sign-in stays valid after it.
//! - **Call [`Session::destroy`] when someone signs out.**
//! - **Serve the application with connection information**, as `axa`'s `server`
//!   module does: `into_make_service_with_connect_info`.
//!
//! By default a session is stored for every visitor, including one who has
//! agreed to nothing. [`SessionOptions::opt_in`] stores it only after
//! [`Session::set_store`], for a service that must ask first.

use std::{fmt, time::Duration};

use axum_session::DatabasePool;
pub use axum_session::{
    Key, ReadOnlySession, SameSite, Session, SessionConfig, SessionError, SessionLayer,
    SessionMode, SessionNullPool, SessionStore,
};
#[cfg(feature = "postgres")]
pub use axum_session_sqlx::{SessionPgPool, SessionPgSession, SessionPgSessionStore};

/// The fewest bytes a key may have. `axum_session` panics on a shorter one, so
/// this crate refuses it first.
pub const MIN_KEY_BYTES: usize = 64;

/// How long a session lasts without being used, by default.
pub const DEFAULT_LIFETIME: Duration = Duration::from_hours(8);

/// How long a long-term session and its cookie last, by default.
pub const DEFAULT_MAX_LIFETIME: Duration = Duration::from_hours(24 * 7);

/// A session failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A key is too short.
    #[error("the session {name} key has {length} bytes and needs at least {MIN_KEY_BYTES}")]
    KeyTooShort {
        /// Which key.
        name: &'static str,
        /// How many bytes it has.
        length: usize,
    },
    /// The signing and database keys are the same.
    #[error("the session signing and database keys must differ")]
    SameKey,
    /// A lifetime that does not fit.
    #[error("the session lifetime is too long")]
    LifetimeTooLong,
    /// The store failed.
    #[error(transparent)]
    Store(#[from] SessionError),
}

/// How a service's sessions behave.
#[derive(Clone)]
pub struct SessionOptions {
    cookie_name: String,
    table_name: String,
    lifetime: Duration,
    max_lifetime: Duration,
    secure: bool,
    opt_in: bool,
    signing_key: Option<Key>,
    database_key: Option<Key>,
}

impl fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Whether the keys are there, and never what they are.
        f.debug_struct("SessionOptions")
            .field("cookie_name", &self.cookie_name)
            .field("table_name", &self.table_name)
            .field("lifetime", &self.lifetime)
            .field("max_lifetime", &self.max_lifetime)
            .field("secure", &self.secure)
            .field("opt_in", &self.opt_in)
            .field("signed", &self.signing_key.is_some())
            .field("encrypted", &self.database_key.is_some())
            .finish()
    }
}

fn key(name: &'static str, bytes: &[u8]) -> Result<Key, Error> {
    if bytes.len() < MIN_KEY_BYTES {
        return Err(Error::KeyTooShort {
            name,
            length: bytes.len(),
        });
    }
    Ok(Key::from(bytes))
}

impl SessionOptions {
    /// Signed cookies over HTTPS, with session data encrypted in the database.
    ///
    /// # Errors
    ///
    /// Fails when a key is under [`MIN_KEY_BYTES`], or the two are the same.
    pub fn production(signing_key: &[u8], database_key: &[u8]) -> Result<Self, Error> {
        let signing = key("signing", signing_key)?;
        let database = key("database", database_key)?;
        if signing_key == database_key {
            return Err(Error::SameKey);
        }
        Ok(Self {
            secure: true,
            signing_key: Some(signing),
            database_key: Some(database),
            ..Self::development()
        })
    }

    /// Unsigned cookies over plain HTTP, for a developer's machine and for tests.
    /// Anyone can forge these, so it does not belong in production.
    #[must_use]
    pub fn development() -> Self {
        Self {
            cookie_name: "session".to_string(),
            table_name: "sessions".to_string(),
            lifetime: DEFAULT_LIFETIME,
            max_lifetime: DEFAULT_MAX_LIFETIME,
            secure: false,
            opt_in: false,
            signing_key: None,
            database_key: None,
        }
    }

    /// Name the cookie.
    #[must_use]
    pub fn cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Name the Postgres table.
    #[must_use]
    pub fn table_name(mut self, name: impl Into<String>) -> Self {
        self.table_name = name.into();
        self
    }

    /// How long a session lasts without being used.
    #[must_use]
    pub fn lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// How long a long-term session and its cookie last.
    #[must_use]
    pub fn max_lifetime(mut self, max_lifetime: Duration) -> Self {
        self.max_lifetime = max_lifetime;
        self
    }

    /// Store a session only after [`Session::set_store`] allows it.
    #[must_use]
    pub fn opt_in(mut self) -> Self {
        self.opt_in = true;
        self
    }

    /// The `axum_session` settings these options describe.
    ///
    /// # Errors
    ///
    /// Fails when a lifetime does not fit `axum_session`'s duration type.
    pub fn config(&self) -> Result<SessionConfig, Error> {
        let duration =
            |value: Duration| time::Duration::try_from(value).map_err(|_| Error::LifetimeTooLong);
        let max_lifetime = duration(self.max_lifetime)?;
        let mut config = SessionConfig::default()
            .with_session_name(self.cookie_name.clone())
            .with_table_name(self.table_name.clone())
            .with_lifetime(duration(self.lifetime)?)
            .with_max_lifetime(max_lifetime)
            .with_max_age(Some(max_lifetime))
            .with_secure(self.secure)
            .with_http_only(true)
            .with_cookie_same_site(SameSite::Lax)
            .with_hashed_ip(false);
        if self.opt_in {
            config = config.with_mode(SessionMode::OptIn);
        }
        if let Some(key) = &self.signing_key {
            config = config.with_key(key.clone());
        }
        if let Some(key) = &self.database_key {
            config = config.with_database_key(key.clone());
        }
        Ok(config)
    }
}

/// A store over `client`.
///
/// # Errors
///
/// Fails when the options do not fit, or the store cannot start.
pub async fn store<T>(client: Option<T>, options: &SessionOptions) -> Result<SessionStore<T>, Error>
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    Ok(SessionStore::new(client, options.config()?).await?)
}

/// A store held in this process. Replicas do not share it, and a restart loses it.
///
/// # Errors
///
/// Fails when the options do not fit.
pub async fn memory_store(
    options: &SessionOptions,
) -> Result<SessionStore<SessionNullPool>, Error> {
    store(None, options).await
}

/// A store in Postgres, creating its table if needed.
///
/// # Errors
///
/// Fails when the options do not fit, or the table cannot be created.
#[cfg(feature = "postgres")]
pub async fn postgres_store(
    pool: sqlx::PgPool,
    options: &SessionOptions,
) -> Result<SessionPgSessionStore, Error> {
    store(Some(pool.into()), options).await
}

/// The layer that gives handlers a [`Session`].
#[must_use]
pub fn layer<T>(store: SessionStore<T>) -> SessionLayer<T>
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    SessionLayer::new(store)
}
