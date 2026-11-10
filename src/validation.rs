//! Model validation, the same for every database, with the shape of Loco's.
//!
//! Implement [`Validatable`] on a model, or on the struct a handler builds from a
//! request, then call `validate()?`. A failure is [`ModelValidationErrors`], which
//! becomes `400` with each field's failures through [`crate::model::ModelError`].
//!
//! With `SeaORM`, implement it on an `ActiveModel` and call `self.validate()?` in
//! `before_save`: there a failure becomes a `DbErr::Custom` whose text is a JSON map
//! of field name to error list.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationErrors};

/// One failure on one field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    /// The rule that failed, such as `email` or `length`.
    pub code: String,
    /// The message, if the rule has one.
    pub message: Option<String>,
    /// The rule's parameters.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub params: HashMap<String, serde_json::Value>,
}

/// Validation failures by field name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("Model validation failed")]
pub struct ModelValidationErrors {
    /// The failures.
    pub errors: BTreeMap<String, Vec<ValidationError>>,
}

impl From<ValidationErrors> for ModelValidationErrors {
    fn from(value: ValidationErrors) -> Self {
        let errors = value
            .field_errors()
            .iter()
            .map(|(field, failures)| {
                let failures = failures
                    .iter()
                    .map(|failure| ValidationError {
                        code: failure.code.to_string(),
                        message: failure.message.as_ref().map(ToString::to_string),
                        params: failure
                            .params
                            .iter()
                            .map(|(name, value)| (name.to_string(), value.clone()))
                            .collect(),
                    })
                    .collect();
                ((*field).to_string(), failures)
            })
            .collect();
        Self { errors }
    }
}

#[cfg(has_seaorm)]
impl From<ModelValidationErrors> for sea_orm::DbErr {
    fn from(errors: ModelValidationErrors) -> Self {
        // Compact on purpose: `code` and `message` only, which is the text
        // Loco produced, so snapshots of these errors do not change.
        let compact: BTreeMap<&String, Vec<_>> = errors
            .errors
            .iter()
            .map(|(field, failures)| {
                let failures = failures
                    .iter()
                    .map(|failure| {
                        serde_json::json!({ "code": failure.code, "message": failure.message })
                    })
                    .collect();
                (field, failures)
            })
            .collect();
        match serde_json::to_string(&compact) {
            Ok(text) => Self::Custom(text),
            Err(error) => Self::Custom(format!("could not encode validation errors: {error}")),
        }
    }
}

/// A model that can validate itself.
pub trait Validatable {
    /// The value to validate, usually a small struct built from this model.
    fn validator(&self) -> Box<dyn Validate>;

    /// Run the validation.
    ///
    /// # Errors
    ///
    /// Returns the failures by field.
    fn validate(&self) -> Result<(), ModelValidationErrors> {
        self.validator()
            .validate()
            .map_err(ModelValidationErrors::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Validate)]
    struct Fields {
        #[validate(length(min = 2, message = "Name is too short."))]
        name: String,
        #[validate(email(message = "invalid email"))]
        email: String,
    }

    struct Model {
        name: String,
        email: String,
    }

    impl Validatable for Model {
        fn validator(&self) -> Box<dyn Validate> {
            Box::new(Fields {
                name: self.name.clone(),
                email: self.email.clone(),
            })
        }
    }

    fn model(name: &str, email: &str) -> Model {
        Model {
            name: name.to_string(),
            email: email.to_string(),
        }
    }

    #[test]
    fn a_valid_model_passes() {
        assert!(model("Alice", "a@example.org").validate().is_ok());
    }

    #[test]
    fn failures_are_reported_by_field() {
        let errors = model("A", "nope").validate().unwrap_err();
        assert_eq!(errors.errors["email"][0].code, "email");
        assert_eq!(
            errors.errors["name"][0].message.as_deref(),
            Some("Name is too short.")
        );
    }

    #[test]
    fn a_rules_parameters_are_kept() {
        let errors = model("A", "a@example.org").validate().unwrap_err();
        assert_eq!(errors.errors["name"][0].params["min"], 2);
    }

    #[cfg(has_seaorm)]
    #[test]
    fn the_database_error_is_compact_json() {
        use sea_orm::DbErr;

        let errors = model("Alice", "nope").validate().unwrap_err();
        let DbErr::Custom(text) = DbErr::from(errors) else {
            panic!("expected a custom error");
        };
        assert_eq!(
            text,
            r#"{"email":[{"code":"email","message":"invalid email"}]}"#
        );
    }
}
