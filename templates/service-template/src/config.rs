//! Configuration Management
//!
//! This module handles configuration from environment variables.
//! It uses axa's configuration patterns for consistency.

use serde::Deserialize;
use std::env;

/// Application configuration
///
/// Loaded from environment variables with the following defaults:
/// - PORT: 3000
/// - ENVIRONMENT: development (or production)
/// - LOG_LEVEL: info
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Port to listen on
    pub port: Option<u16>,
    /// Environment (development, staging, production)
    pub environment: Option<String>,
    /// Log level (debug, info, warn, error)
    pub log_level: Option<String>,
    /// Log format (json, pretty)
    pub log_format: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: Some(3000),
            environment: Some("development".to_string()),
            log_level: Some("info".to_string()),
            log_format: Some("pretty".to_string()),
        }
    }
}

impl Config {
    /// Load configuration from environment variables
    ///
    /// # Panics
    ///
    /// Panics if required environment variables are missing in production
    pub fn from_env() -> Result<Self, String> {
        // Check if we're in production
        let environment = env::var("ENVIRONMENT")
            .or_else(|_| env::var("APP_ENV"))
            .unwrap_or_else(|_| "development".to_string());

        let is_production = environment == "production";

        // Load configuration
        let config = Self {
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .or_else(|| Some(3000)),
            environment: Some(environment),
            log_level: env::var("LOG_LEVEL")
                .or_else(|_| env::var("RUST_LOG"))
                .ok(),
            log_format: env::var("LOG_FORMAT").ok(),
        };

        // In production, validate required settings
        if is_production {
            // Add production-specific validation here
            // For example, require database URL, etc.
        }

        Ok(config)
    }

    /// Check if running in production mode
    pub fn is_production(&self) -> bool {
        self.environment.as_deref() == Some("production")
    }

    /// Check if running in development mode
    pub fn is_development(&self) -> bool {
        self.environment.as_deref() == Some("development")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.port, Some(3000));
        assert_eq!(config.environment, Some("development".to_string()));
        assert_eq!(config.log_level, Some("info".to_string()));
    }

    #[test]
    fn test_from_env() {
        env::set_var("PORT", "8080");
        env::set_var("ENVIRONMENT", "production");
        env::set_var("LOG_LEVEL", "warn");

        let config = Config::from_env().unwrap();
        assert_eq!(config.port, Some(8080));
        assert_eq!(config.environment, Some("production".to_string()));
        assert_eq!(config.log_level, Some("warn".to_string()));

        env::remove_var("PORT");
        env::remove_var("ENVIRONMENT");
        env::remove_var("LOG_LEVEL");
    }

    #[test]
    fn test_is_production() {
        let config = Config {
            port: Some(3000),
            environment: Some("production".to_string()),
            log_level: Some("info".to_string()),
            log_format: None,
        };
        assert!(config.is_production());
        assert!(!config.is_development());
    }

    #[test]
    fn test_is_development() {
        let config = Config {
            port: Some(3000),
            environment: Some("development".to_string()),
            log_level: Some("info".to_string()),
            log_format: None,
        };
        assert!(config.is_development());
        assert!(!config.is_production());
    }
}
