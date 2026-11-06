//! Which environment a service runs in.
//!
//! `axa` re-exports this crate as `axa::environment`. It has no dependencies,
//! so a migration crate can use it without depending on `axa`, and a safety
//! guard cannot drift from the service's own order of lookup.
//!
//! ```
//! use axa_environment::Environment;
//!
//! // `name` gives what is set, and `from_env` what that means.
//! let guarded = Environment::from_env().is_production_like();
//! # let _ = (guarded, Environment::name());
//! ```
//!
//! With nothing set, a debug build is development and a release build is
//! production, so a release binary started with no variables gets the strict
//! rules. The build profile is read from `cfg!(debug_assertions)`, so a release
//! profile that turns debug assertions on is treated as a debug build.

use std::fmt;

/// The variables that name the environment, in the order they are read.
pub const VARIABLES: [&str; 2] = ["APP_ENV", "LOCO_ENV"];

/// The environment, taken from `APP_ENV` (or the older `LOCO_ENV`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Environment {
    /// Local development; the default for a debug build.
    Development,
    /// Automated tests.
    Test,
    /// A real deployment; the default for a release build.
    Production,
    /// Any other named environment. Treated as production for defaults.
    Other(String),
}

impl Environment {
    /// Parse a name. Accepts the short forms `dev`, `prod`. A blank name is
    /// the same as no name: see [`Environment::resolve`].
    #[must_use]
    pub fn parse(name: &str) -> Self {
        Self::resolve(Some(name), cfg!(debug_assertions))
    }

    /// The environment for `name`, as a pure function of its arguments.
    /// `debug` says whether this is a debug build. With no name, or a blank
    /// one, a debug build is development and any other build is production.
    ///
    /// # Examples
    ///
    /// ```
    /// use axa_environment::Environment;
    ///
    /// assert_eq!(Environment::resolve(None, true), Environment::Development);
    /// assert_eq!(Environment::resolve(None, false), Environment::Production);
    /// assert_eq!(Environment::resolve(Some("test"), false), Environment::Test);
    /// ```
    #[must_use]
    pub fn resolve(name: Option<&str>, debug: bool) -> Self {
        let name = name.unwrap_or_default().trim().to_ascii_lowercase();
        match (name.as_str(), debug) {
            ("development" | "dev", _) | ("", true) => Self::Development,
            ("test", _) => Self::Test,
            ("production" | "prod", _) | ("", false) => Self::Production,
            (other, _) => Self::Other(other.to_string()),
        }
    }

    /// The name set in the process environment, read as
    /// [`Environment::name_from`] reads it.
    #[must_use]
    pub fn name() -> Option<String> {
        Self::name_from(|variable| std::env::var(variable).ok())
    }

    /// The first non-blank name among [`VARIABLES`], read through `lookup`.
    ///
    /// # Examples
    ///
    /// ```
    /// use axa_environment::Environment;
    ///
    /// let name = Environment::name_from(|variable| {
    ///     (variable == "LOCO_ENV").then(|| "staging".to_string())
    /// });
    /// assert_eq!(name.as_deref(), Some("staging"));
    /// ```
    #[must_use]
    pub fn name_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
        VARIABLES
            .iter()
            .find_map(|variable| lookup(variable).filter(|name| !name.trim().is_empty()))
    }

    /// Read the environment from `APP_ENV`, then `LOCO_ENV`.
    #[must_use]
    pub fn from_env() -> Self {
        Self::resolve(Self::name().as_deref(), cfg!(debug_assertions))
    }

    /// True for anything that is not development or test, so an unknown
    /// name gets the stricter behaviour.
    #[must_use]
    pub fn is_production_like(&self) -> bool {
        !matches!(self, Self::Development | Self::Test)
    }

    /// Whether a command that destroys data, such as `db down`, `db reset`, or
    /// `db truncate`, may run: only in development or test, and only when
    /// `confirmed` is `"true"`, so that running one takes a choice as well as a
    /// typed command.
    ///
    /// `axa::cli::parse` applies this before a service runs anything. Call it
    /// yourself for a destructive path that does not come through there.
    #[must_use]
    pub fn may_destroy_data(&self, confirmed: Option<&str>) -> bool {
        !self.is_production_like() && confirmed == Some("true")
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

    fn lookup(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs.to_vec();
        move |variable| {
            pairs
                .iter()
                .find(|(name, _)| *name == variable)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn parses_names_and_short_forms() {
        assert_eq!(Environment::parse("prod"), Environment::Production);
        assert_eq!(Environment::parse("DEV"), Environment::Development);
        assert_eq!(Environment::parse("test"), Environment::Test);
    }

    #[test]
    fn names_round_trip() {
        for name in ["development", "test", "production", "staging"] {
            assert_eq!(Environment::parse(name).to_string(), name);
        }
    }

    #[test]
    fn unknown_environment_is_production_like() {
        assert!(Environment::parse("staging").is_production_like());
        assert!(!Environment::Test.is_production_like());
    }

    #[test]
    fn app_env_is_read_before_loco_env() {
        let both = lookup(&[("APP_ENV", "production"), ("LOCO_ENV", "development")]);
        assert_eq!(Environment::name_from(both).as_deref(), Some("production"));
        let older = lookup(&[("LOCO_ENV", "test")]);
        assert_eq!(Environment::name_from(older).as_deref(), Some("test"));
        assert_eq!(Environment::name_from(lookup(&[])), None);
    }

    #[test]
    fn a_blank_name_counts_as_unset() {
        let blank = lookup(&[("APP_ENV", "  "), ("LOCO_ENV", "production")]);
        assert_eq!(Environment::name_from(blank).as_deref(), Some("production"));
        assert_eq!(Environment::name_from(lookup(&[("APP_ENV", "")])), None);
    }

    #[test]
    fn a_debug_build_with_no_name_is_development() {
        assert_eq!(Environment::resolve(None, true), Environment::Development);
        assert_eq!(
            Environment::resolve(Some("  "), true),
            Environment::Development
        );
    }

    #[test]
    fn a_release_build_with_no_name_is_production() {
        assert_eq!(Environment::resolve(None, false), Environment::Production);
        assert_eq!(
            Environment::resolve(Some("  "), false),
            Environment::Production
        );
        assert!(Environment::resolve(None, false).is_production_like());
    }

    #[test]
    fn a_name_means_the_same_in_every_build() {
        for debug in [true, false] {
            assert_eq!(
                Environment::resolve(Some("dev"), debug),
                Environment::Development
            );
            assert_eq!(Environment::resolve(Some("test"), debug), Environment::Test);
            assert_eq!(
                Environment::resolve(Some("staging"), debug),
                Environment::Other("staging".to_string())
            );
        }
    }

    #[test]
    fn this_crate_has_no_dependencies() {
        let manifest = include_str!("../Cargo.toml");
        assert!(!manifest.contains("dependencies]"), "{manifest}");
    }

    #[test]
    fn data_is_destroyed_only_in_development_or_test_and_only_when_confirmed() {
        for environment in [Environment::Development, Environment::Test] {
            assert!(environment.may_destroy_data(Some("true")));
            assert!(!environment.may_destroy_data(None));
            assert!(!environment.may_destroy_data(Some("yes")));
        }
        for environment in [
            Environment::Production,
            Environment::Other("staging".to_string()),
            Environment::Other("Test ".to_string()),
        ] {
            assert!(!environment.may_destroy_data(Some("true")));
        }
    }

    #[test]
    fn parsing_a_blank_name_follows_the_build() {
        assert_eq!(
            Environment::parse(" "),
            Environment::resolve(None, cfg!(debug_assertions))
        );
    }
}
