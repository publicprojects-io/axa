//! The command line every service shares.
//!
//! A service parses [`Cli`] in `main` and matches on [`Command`] itself, so
//! the flow stays visible in its own code:
//!
//! ```text
//! service start [--worker | --all] [--binding ADDR] [--port PORT]
//! service db migrate | down [STEPS] | reset | status | truncate
//! service task [NAME] [KEY:VALUE ...]
//! ```

use clap::{Parser, Subcommand};

use crate::{Error, Result, task::Vars};

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
        #[arg(short, long)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn command_line_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
