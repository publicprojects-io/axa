//! Model-layer errors for `SeaORM` services.

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
    /// A database failure.
    #[error(transparent)]
    DbErr(#[from] sea_orm::DbErr),
    /// Any other failure.
    #[error(transparent)]
    Any(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// A free-form failure.
    #[error("{0}")]
    Message(String),
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
