//! Which environment a service is running in.

use std::fmt;

/// The environment, taken from `APP_ENV` (or the older `LOCO_ENV`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Environment {
    /// Local development; the default when nothing is set.
    Development,
    /// Automated tests.
    Test,
    /// A real deployment.
    Production,
    /// Any other named environment. Treated as production for defaults.
    Other(String),
}

impl Environment {
    /// Parse a name. Accepts the short forms `dev`, `prod`.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "development" | "dev" | "" => Self::Development,
            "test" => Self::Test,
            "production" | "prod" => Self::Production,
            other => Self::Other(other.to_string()),
        }
    }

    /// Read the environment from `APP_ENV`, then `LOCO_ENV`.
    #[must_use]
    pub fn from_env() -> Self {
        std::env::var("APP_ENV")
            .or_else(|_| std::env::var("LOCO_ENV"))
            .map_or(Self::Development, |name| Self::parse(&name))
    }

    /// True for anything that is not development or test, so an unknown
    /// name gets the stricter behaviour.
    #[must_use]
    pub fn is_production_like(&self) -> bool {
        !matches!(self, Self::Development | Self::Test)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Production => "production",
            Self::Other(name) => name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_short_forms() {
        assert_eq!(Environment::parse("prod"), Environment::Production);
        assert_eq!(Environment::parse("DEV"), Environment::Development);
        assert_eq!(Environment::parse("test"), Environment::Test);
    }

    #[test]
    fn unknown_environment_is_production_like() {
        assert!(Environment::parse("staging").is_production_like());
        assert!(!Environment::Test.is_production_like());
    }
}
