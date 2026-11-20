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
    /// A `SeaORM` failure, from any database.
    #[cfg(has_seaorm)]
    #[error(transparent)]
    DbErr(#[from] sea_orm::DbErr),
    /// A `sqlx` failure, from any database.
    #[cfg(has_sqlx)]
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// A Diesel failure, from any database's Diesel crate.
    #[cfg(has_diesel)]
    #[error(transparent)]
    Diesel(#[from] crate::error::DieselError),
    /// A Toasty failure, from any database.
    #[cfg(has_toasty)]
    #[error(transparent)]
    Toasty(#[from] crate::error::toasty_crate::Error),
    /// Any other failure.
    #[error(transparent)]
    Any(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// A free-form failure.
    #[error("{0}")]
    Message(String),
}

/// A Diesel query's own error, as Diesel's functions return it.
#[cfg(has_diesel)]
impl From<crate::error::diesel_crates::diesel::result::Error> for ModelError {
    fn from(error: crate::error::diesel_crates::diesel::result::Error) -> Self {
        Self::Diesel(crate::error::DieselError::Query(error))
    }
}

/// One `From` for each Diesel crate's error.
macro_rules! model_from_diesel_crate {
    ($($feature:literal => $krate:ident),*) => {$(
        #[cfg(feature = $feature)]
        impl From<$krate::Error> for ModelError {
            fn from(error: $krate::Error) -> Self {
                Self::Diesel(error.into())
            }
        }
    )*};
}
model_from_diesel_crate!(
    "postgres-diesel" => axa_postgres_diesel,
    "mysql-diesel" => axa_mysql_diesel,
    "sqlite-diesel" => axa_sqlite_diesel
);

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

#[cfg(test)]
mod tests {
    use super::ModelError;

    #[test]
    fn each_constructor_keeps_the_text_it_was_given() {
        let wrapped = ModelError::wrap(std::io::Error::other("disk full"));
        assert!(matches!(wrapped, ModelError::Any(_)));
        assert_eq!(wrapped.to_string(), "disk full");
        let kept = ModelError::to_msg(std::io::Error::other("disk full"));
        assert!(matches!(&kept, ModelError::Message(text) if text == "disk full"));
        assert_eq!(ModelError::msg("no stock").to_string(), "no stock");
        assert_eq!(ModelError::EntityNotFound.to_string(), "Entity not found");
        assert_eq!(
            ModelError::EntityAlreadyExists.to_string(),
            "Entity already exists"
        );
    }

    #[cfg(has_diesel)]
    #[test]
    fn a_diesel_query_error_is_the_diesel_variant() {
        use crate::error::diesel_crates::diesel::result::Error as Query;
        let error: ModelError = Query::NotFound.into();
        assert!(
            matches!(
                error,
                ModelError::Diesel(crate::error::DieselError::Query(Query::NotFound))
            ),
            "{error:?}"
        );
    }

    #[cfg(feature = "postgres-diesel")]
    #[test]
    fn a_diesel_crates_own_error_is_the_diesel_variant_with_its_text() {
        let error: ModelError = axa_postgres_diesel::Error::Migration("0001 failed".into()).into();
        assert!(matches!(error, ModelError::Diesel(_)), "{error:?}");
        assert!(error.to_string().contains("0001 failed"), "{error}");
    }
}
