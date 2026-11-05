//! The command line every service shares.
//!
//! A service parses [`Cli`] in `main` and matches on [`Command`] itself, so
//! the flow stays visible in its own code:
//!
//! ```text
//! service start [--worker | --all] [--binding ADDR] [--port PORT]
//! service db migrate | down [STEPS] | reset | status | truncate
//! service task [NAME] [KEY:VALUE ...]
//! service check
//! ```

use clap::{Parser, Subcommand};

use crate::{
    Environment, Error, Result,
    config::{Config, Defaults, redact},
    task::Vars,
};

/// Parsed command line.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
    /// Environment, overriding `APP_ENV`.
    #[arg(short, long, global = true)]
    pub environment: Option<String>,
}

/// A subcommand.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the service.
    Start {
        /// Run only the job worker.
        #[arg(short, long, conflicts_with = "all")]
        worker: bool,
        /// Run the server and the job worker together.
        #[arg(short, long, alias = "server-and-worker")]
        all: bool,
        /// Override the listening address.
        #[arg(short, long)]
        binding: Option<String>,
        /// Override the listening port.
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Manage the database.
    Db {
        /// Database action.
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Run a named task, or list them when no name is given.
    Task {
        /// The task's name.
        name: Option<String>,
        /// `key:value` arguments.
        params: Vec<String>,
    },
    /// Validate the configuration, print the effective settings with secrets
    /// redacted, and exit. Touches no database or other dependency.
    Check,
}

/// A database action.
#[derive(Debug, Subcommand)]
pub enum DbCommand {
    /// Apply pending migrations.
    Migrate,
    /// Revert migrations.
    Down {
        /// How many to revert.
        #[arg(default_value_t = 1)]
        steps: u32,
    },
    /// Drop everything and reapply all migrations.
    Reset,
    /// Show which migrations have run.
    Status,
    /// Delete all rows.
    Truncate,
}

/// Parse `key:value` task arguments.
///
/// # Errors
///
/// Fails on an argument with no `:`.
pub fn parse_vars(params: &[String]) -> Result<Vars> {
    let pairs = params
        .iter()
        .map(|param| {
            param
                .split_once(':')
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .ok_or_else(|| Error::Message(format!("expected key:value, got {param}")))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Vars::from_cli_args(pairs))
}

/// Run the `check` command: read the configuration as a real start would, and
/// print the report. Call it before connecting to anything.
///
/// This covers the settings `axa` reads. A service's own settings are its to
/// check, and [`redact`] is there for printing them.
///
/// # Errors
///
/// Fails when the configuration is refused, so `main` exits non-zero.
pub fn check(environment: &Environment, defaults: &Defaults) -> Result<()> {
    let report = check_report(environment, defaults, |name| std::env::var(name).ok())?;
    print!("{report}");
    Ok(())
}

/// The text the `check` command prints, reading settings through `lookup`.
/// Each line is one setting. Anything secret-shaped is replaced by
/// [`crate::config::REDACTED`].
///
/// # Errors
///
/// Fails when the configuration is refused.
pub fn check_report(
    environment: &Environment,
    defaults: &Defaults,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String> {
    let config = Config::from_lookup(environment, defaults, lookup)?;
    let mut lines = vec![format!(
        "APP_ENV={}",
        redact("APP_ENV", &environment.to_string())
    )];
    let settings = config.settings().into_iter();
    lines.extend(settings.map(|(name, value)| format!("{name}={value}")));
    let warnings = config.logger.warnings.iter();
    lines.extend(warnings.map(|warning| format!("warning: {warning}")));
    Ok(lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::REDACTED;

    #[test]
    fn task_arguments_keep_colons_in_values() {
        let vars = parse_vars(&["url:http://x:1".to_string()]).unwrap();
        assert_eq!(vars.cli_arg("url").unwrap(), "http://x:1");
    }

    #[test]
    fn task_argument_without_a_colon_is_refused() {
        assert!(parse_vars(&["oops".to_string()]).is_err());
    }

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn start_takes_its_options() {
        let cli = parse(&[
            "svc",
            "start",
            "--all",
            "--port",
            "9000",
            "--binding",
            "0.0.0.0",
        ]);
        let Command::Start {
            worker,
            all,
            binding,
            port,
        } = cli.command
        else {
            panic!("expected start");
        };
        assert!(!worker);
        assert!(all);
        assert_eq!(binding.as_deref(), Some("0.0.0.0"));
        assert_eq!(port, Some(9000));
    }

    #[test]
    fn the_older_spelling_of_all_still_works() {
        let cli = Cli::try_parse_from(["svc", "start", "--server-and-worker"]).unwrap();
        assert!(matches!(cli.command, Command::Start { all: true, .. }));
    }

    #[test]
    fn worker_and_all_conflict() {
        assert!(Cli::try_parse_from(["svc", "start", "--worker", "--all"]).is_err());
    }

    #[test]
    fn database_commands_parse() {
        let cli = parse(&["svc", "db", "down", "3"]);
        assert!(matches!(
            cli.command,
            Command::Db {
                command: DbCommand::Down { steps: 3 }
            }
        ));
        let cli = parse(&["svc", "db", "down"]);
        assert!(matches!(
            cli.command,
            Command::Db {
                command: DbCommand::Down { steps: 1 }
            }
        ));
    }

    #[test]
    fn task_takes_a_name_and_parameters() {
        let cli = parse(&["svc", "task", "seed", "a:1", "b:2"]);
        let Command::Task { name, params } = cli.command else {
            panic!("expected task");
        };
        assert_eq!(name.as_deref(), Some("seed"));
        assert_eq!(params, ["a:1", "b:2"]);
    }

    #[test]
    fn the_environment_flag_works_after_the_subcommand() {
        let cli = parse(&["svc", "start", "--environment", "production"]);
        assert_eq!(cli.environment.as_deref(), Some("production"));
    }

    #[test]
    fn check_takes_no_arguments() {
        assert!(matches!(parse(&["svc", "check"]).command, Command::Check));
        assert!(Cli::try_parse_from(["svc", "check", "extra"]).is_err());
    }

    fn report(pairs: &[(&'static str, &'static str)], environment: &Environment) -> Result<String> {
        let pairs = pairs.to_vec();
        let defaults = Defaults {
            database: "svc",
            ..Defaults::default()
        };
        check_report(environment, &defaults, move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        })
    }

    #[test]
    fn check_prints_the_effective_settings() {
        let text = report(
            &[("DATABASE_URL", "postgres://h/db"), ("PORT", "8000")],
            &Environment::Production,
        )
        .unwrap();
        for line in [
            "APP_ENV=production",
            "PORT=8000",
            "BINDING=0.0.0.0",
            "HTTP_HSTS=true",
            "REQUEST_TIMEOUT_SECONDS=60",
            "WORKER_MODE=BackgroundQueue",
            "dangerously_truncate=false",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} is not in {text}");
        }
    }

    #[test]
    fn check_never_prints_a_password() {
        let text = report(
            &[
                ("DATABASE_URL", "postgres://me:hunter2@h/db?sslpassword=abc"),
                ("HOST", "https://me:hunter3@example.org"),
            ],
            &Environment::Production,
        )
        .unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("hunter3"), "{text}");
        assert!(!text.contains("abc"), "{text}");
        assert!(
            text.contains(&format!("postgres://me:{REDACTED}@h/db")),
            "{text}"
        );
    }

    #[test]
    fn check_refuses_what_a_start_would_refuse() {
        let error = report(&[], &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("DATABASE_URL"));
        let pairs = [("DATABASE_URL", "postgres://h/db"), ("HTTP_HSTS", "tru")];
        let error = report(&pairs, &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("HTTP_HSTS"));
    }

    #[test]
    fn check_shows_warnings_outside_production() {
        let text = report(&[("HTTP_HSTS", "tru")], &Environment::Development).unwrap();
        assert!(text.contains("warning: HTTP_HSTS"), "{text}");
    }

    #[test]
    fn command_line_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
