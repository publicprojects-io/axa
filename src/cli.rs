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

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind};

use crate::{
    Environment, Error, Result,
    config::{Config, Defaults, redact},
    task::Vars,
};

/// The variable that confirms a command which destroys data. It must be `true`.
pub const DESTROY_DATA_VAR: &str = "AXA_DESTROY_DATA";

/// Parsed command line.
///
/// Read it with [`parse`], which gives `--help` and `--version` the service's
/// own name. `Cli::parse()` works too, and says nothing about the service. Both
/// refuse a command that must not destroy data here, and so does
/// `Cli::try_parse()`. `Cli::parse_from` and `Cli::try_parse_from` read no process
/// arguments, are for tests, and refuse nothing.
#[derive(Debug, Args)]
#[command(about = None, long_about = None)]
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

impl DbCommand {
    /// Whether this destroys data: `down`, `reset`, and `truncate`.
    #[must_use]
    pub const fn destroys_data(&self) -> bool {
        matches!(self, Self::Down { .. } | Self::Reset | Self::Truncate)
    }
}

/// Why `cli` must not run here, or `None` if it may.
///
/// `process` is the environment the process runs in, whatever `--environment`
/// says, so that the flag cannot talk a production process out of this: both
/// must allow it.
fn refusal(cli: &Cli, process: &Environment, confirmed: Option<&str>) -> Option<String> {
    let Command::Db { command } = &cli.command else {
        return None;
    };
    let chosen = cli.environment.as_deref().map(Environment::parse);
    let allowed = process.may_destroy_data(confirmed)
        && chosen
            .as_ref()
            .is_none_or(|environment| environment.may_destroy_data(confirmed));
    (command.destroys_data() && !allowed).then(|| {
        format!(
            "refused: `db down`, `db reset`, and `db truncate` destroy data, so they run \
             only in development or test, with {DESTROY_DATA_VAR}=true"
        )
    })
}

/// [`refusal`] for this process.
fn refused(cli: &Cli) -> Option<String> {
    let confirmed = std::env::var(DESTROY_DATA_VAR).ok();
    refusal(cli, &Environment::from_env(), confirmed.as_deref())
}

/// `cli`, unless it must not run here, which exits.
fn allowed(cli: Cli) -> Cli {
    if let Some(message) = refused(&cli) {
        eprintln!("{message}");
        std::process::exit(1);
    }
    cli
}

// Written out rather than derived, so that `Cli::parse()` cannot leave the guard out:
// a service calling it is not told to read this.
impl CommandFactory for Cli {
    fn command() -> clap::Command {
        Self::augment_args(clap::Command::new(env!("CARGO_PKG_NAME")))
    }

    fn command_for_update() -> clap::Command {
        Self::augment_args_for_update(clap::Command::new(env!("CARGO_PKG_NAME")))
    }
}

impl Parser for Cli {
    fn parse() -> Self {
        let matches = Self::command().get_matches();
        allowed(Self::from_arg_matches(&matches).unwrap_or_else(|error| error.exit()))
    }

    fn try_parse() -> std::result::Result<Self, clap::Error> {
        let cli = Self::from_arg_matches(&Self::command().try_get_matches()?)?;
        match refused(&cli) {
            Some(message) => Err(Self::command().error(ErrorKind::InvalidValue, message)),
            None => Ok(cli),
        }
    }
}

/// The command line under a service's own name, version, and description.
fn command(name: &'static str, version: &'static str, about: &'static str) -> clap::Command {
    Cli::command().name(name).version(version).about(about)
}

/// Read the process's arguments, answering `--help` and `--version` for the
/// service rather than for this crate. Cargo fills these in for the crate that
/// writes them, so they must be written in the service:
///
/// ```no_run
/// let cli = axa::cli::parse(
///     env!("CARGO_PKG_NAME"),
///     env!("CARGO_PKG_VERSION"),
///     env!("CARGO_PKG_DESCRIPTION"),
/// );
/// ```
///
/// Exits as `clap` does on a bad argument, on `--help`, and on `--version`.
/// Exits non-zero, before the service runs anything, on a command that destroys
/// data outside development and test, or without [`DESTROY_DATA_VAR`] set to
/// `true`. `Cli::parse()` refuses the same, so a service cannot leave the guard out.
#[must_use]
pub fn parse(name: &'static str, version: &'static str, about: &'static str) -> Cli {
    let matches = command(name, version, about).get_matches();
    allowed(Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit()))
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
            &[
                ("DATABASE_URL", "postgres://localhost/db"),
                ("PORT", "8000"),
            ],
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
                (
                    "DATABASE_URL",
                    "postgres://me:hunter2@localhost/db?sslpassword=abc",
                ),
                ("HOST", "https://me:hunter3@example.org"),
            ],
            &Environment::Production,
        )
        .unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("hunter3"), "{text}");
        assert!(!text.contains("abc"), "{text}");
        assert!(
            text.contains(&format!("postgres://me:{REDACTED}@localhost/db")),
            "{text}"
        );
    }

    #[test]
    fn check_refuses_what_a_start_would_refuse() {
        let error = report(&[], &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("DATABASE_URL"));
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/db"),
            ("HTTP_HSTS", "tru"),
        ];
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
        Cli::command().debug_assert();
    }

    #[test]
    fn help_and_version_speak_for_the_service() {
        let mut command = command("case-folder", "7.8.9", "Holds case folders.");
        assert_eq!(command.render_version(), "case-folder 7.8.9\n");
        let help = command.render_long_help().to_string();
        assert!(help.contains("Holds case folders."), "{help}");
        assert!(!help.contains("Axum"), "{help}");
    }

    #[test]
    fn only_down_reset_and_truncate_destroy_data() {
        assert!(DbCommand::Down { steps: 1 }.destroys_data());
        assert!(DbCommand::Reset.destroys_data());
        assert!(DbCommand::Truncate.destroys_data());
        assert!(!DbCommand::Migrate.destroys_data());
        assert!(!DbCommand::Status.destroys_data());
    }

    #[test]
    fn a_destructive_command_is_refused_outside_development_and_test() {
        for process in [
            Environment::Production,
            Environment::Other("staging".to_string()),
        ] {
            for command in ["reset", "truncate", "down"] {
                let cli = parse(&["svc", "db", command]);
                assert!(refusal(&cli, &process, Some("true")).is_some(), "{command}");
            }
        }
    }

    #[test]
    fn a_destructive_command_needs_confirming() {
        let cli = parse(&["svc", "db", "truncate"]);
        assert!(refusal(&cli, &Environment::Development, None).is_some());
        assert!(refusal(&cli, &Environment::Test, Some("yes")).is_some());
        assert!(refusal(&cli, &Environment::Development, Some("true")).is_none());
        assert!(refusal(&cli, &Environment::Test, Some("true")).is_none());
    }

    #[test]
    fn the_environment_flag_cannot_lift_the_refusal() {
        let cli = parse(&["svc", "db", "reset", "--environment", "test"]);
        let refused = refusal(&cli, &Environment::Production, Some("true"));
        assert!(refused.is_some());
        // It can only add to it.
        let cli = parse(&["svc", "db", "reset", "--environment", "production"]);
        let refused = refusal(&cli, &Environment::Development, Some("true"));
        assert!(refused.is_some());
    }

    #[test]
    fn other_commands_are_never_refused() {
        for args in [
            &["svc", "db", "migrate"][..],
            &["svc", "db", "status"],
            &["svc", "check"],
            &["svc", "start"],
        ] {
            let cli = parse(args);
            assert_eq!(refusal(&cli, &Environment::Production, None), None);
        }
    }

    #[test]
    fn nothing_says_this_crate_is_the_service() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("Axum"), "{help}");
        assert!(Cli::try_parse_from(["svc", "--version"]).is_err());
    }

    #[test]
    fn the_written_out_parser_reads_what_the_derived_one_did() {
        let cli = parse(&["svc", "--environment", "test", "db", "reset"]);
        assert_eq!(cli.environment.as_deref(), Some("test"));
        assert!(matches!(
            cli.command,
            Command::Db {
                command: DbCommand::Reset
            }
        ));
        assert!(Cli::try_parse_from(["svc"]).is_err());
        assert!(Cli::command().get_about().is_none());
    }
}
