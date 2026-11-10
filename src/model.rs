//! Model-layer errors, the same for every database.
//!
//! A model function returns [`ModelResult`], and `?` turns the database's own error
//! into [`ModelError`] for whichever database features are on. [`crate::Error`] turns
//! a `ModelError` into the response: `404`, `409`, `400` with each field's failures,
//! `409` for a violated unique or foreign key constraint, and otherwise `500`.

/// A failure in the model layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ModelError {
    /// The row already exists. Becomes `409`.
    #[error("Entity already exists")]
    EntityAlreadyExists,
    /// The row does not exist. Becomes `404`.
    #[error("Entity not found")]
    EntityNotFound,
    /// The model failed validation. Becomes `400`.
    #[error(transparent)]
    Validation(#[from] crate::validation::ModelValidationErrors),
    /// A `SeaORM` failure.
    #[cfg(feature = "sea-orm")]
    #[error(transparent)]
    DbErr(#[from] sea_orm::DbErr),
    /// A `sqlx` failure.
    #[cfg(feature = "sqlx")]
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// A Diesel failure.
    #[cfg(feature = "diesel")]
    #[error(transparent)]
    Diesel(#[from] axa_diesel::Error),
    /// A Toasty failure.
    #[cfg(feature = "toasty")]
    #[error(transparent)]
    Toasty(#[from] axa_toasty::Error),
    /// Any other failure.
    #[error(transparent)]
    Any(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// A free-form failure.
    #[error("{0}")]
    Message(String),
}

/// A Diesel query's own error, as Diesel's functions return it.
#[cfg(feature = "diesel")]
impl From<axa_diesel::diesel::result::Error> for ModelError {
    fn from(error: axa_diesel::diesel::result::Error) -> Self {
        Self::Diesel(axa_diesel::Error::Query(error))
    }
}

/// Result alias using [`ModelError`].
pub type ModelResult<T, E = ModelError> = std::result::Result<T, E>;

impl ModelError {
    /// Wrap any error.
    #[must_use]
    pub fn wrap(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Any(Box::new(err))
    }

    /// Keep only an error's text.
    #[must_use]
    pub fn to_msg(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Message(err.to_string())
    }

    /// Build from text.
    #[must_use]
    pub fn msg(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}
