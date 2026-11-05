//! The command line every service shares.

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

    #[test]
    fn command_line_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
