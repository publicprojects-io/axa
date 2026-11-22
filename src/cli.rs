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
//! service health [--ready] [--port PORT]
//! ```

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

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
    /// Ask the running server on this machine whether it is live, or with
    /// `--ready` ready, for an image with no `curl`. Exits non-zero if not.
    Health {
        /// Ask `/_health/ready` rather than `/_health/live`.
        #[arg(short, long)]
        ready: bool,
        /// Override the port to ask on.
        #[arg(short, long)]
        port: Option<u16>,
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

/// Why reading the command line stopped.
#[derive(Debug)]
enum Stop {
    /// A bad argument, `--help`, or `--version`, for `clap` to answer.
    Clap(clap::Error),
    /// A command [`refusal`] keeps from running here.
    Refused(String),
}

/// Read `args` with `command`, then refuse what must not run in `process`, given the
/// value of [`DESTROY_DATA_VAR`]. Every reader of the process's own command line goes
/// through this, so none can leave the refusal out.
fn read_from<I, T>(
    command: clap::Command,
    args: I,
    process: &Environment,
    confirmed: Option<&str>,
) -> std::result::Result<Cli, Stop>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let matches = command.try_get_matches_from(args).map_err(Stop::Clap)?;
    let cli = Cli::from_arg_matches(&matches).map_err(Stop::Clap)?;
    match refusal(&cli, process, confirmed) {
        Some(message) => Err(Stop::Refused(message)),
        None => Ok(cli),
    }
}

/// [`read_from`] on this process's arguments and environment.
fn read(command: clap::Command) -> std::result::Result<Cli, Stop> {
    let confirmed = std::env::var(DESTROY_DATA_VAR).ok();
    read_from(
        command,
        std::env::args_os(),
        &Environment::from_env(),
        confirmed.as_deref(),
    )
}

/// [`read`], exiting as `clap` does on a bad argument, and with `1` on a refusal.
fn read_or_exit(command: clap::Command) -> Cli {
    match read(command) {
        Ok(cli) => cli,
        Err(Stop::Clap(error)) => error.exit(),
        Err(Stop::Refused(message)) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
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
        read_or_exit(Self::command())
    }

    fn try_parse() -> std::result::Result<Self, clap::Error> {
        read(Self::command()).map_err(|stop| match stop {
            Stop::Clap(error) => error,
            Stop::Refused(message) => Self::command().error(ErrorKind::InvalidValue, message),
        })
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
/// `true`. `Cli::parse()` refuses the same. A service that flattens [`Command`] into a
/// parser of its own has neither, and must call `Environment::may_destroy_data` itself.
#[must_use]
pub fn parse(name: &'static str, version: &'static str, about: &'static str) -> Cli {
    read_or_exit(command(name, version, about))
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
    check_with(environment, defaults, |name| std::env::var(name).ok())
}

/// [`check`], reading settings through `lookup` rather than the process environment,
/// so that a test, or a service that reads its settings from elsewhere, can run it.
///
/// # Errors
///
/// As [`check`].
pub fn check_with(
    environment: &Environment,
    defaults: &Defaults,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    let report = check_report(environment, defaults, lookup)?;
    print!("{report}");
    Ok(())
}

/// How long `health` waits for the server, from connecting to its answer.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

/// The `health` command: ask the server on this machine, at the configured binding
/// and its internal port if it has one, or else its port, for [`crate::health::LIVE`], or [`crate::health::READY`] with `ready`.
/// A binding to every address, such as `0.0.0.0`, is asked on loopback.
///
/// # Errors
///
/// Fails when the configuration is refused, the server cannot be reached in
/// [`HEALTH_TIMEOUT`], or it answers other than `2xx`, so `main` exits non-zero.
pub async fn health(
    environment: &Environment,
    defaults: &Defaults,
    ready: bool,
    port: Option<u16>,
) -> Result<()> {
    health_from(environment, defaults, ready, port, |name| {
        std::env::var(name).ok()
    })
    .await
}

/// [`health`], reading settings through `lookup` rather than the process environment.
///
/// # Errors
///
/// As [`health`].
pub async fn health_from(
    environment: &Environment,
    defaults: &Defaults,
    ready: bool,
    port: Option<u16>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    let config = Config::from_lookup(environment, defaults, lookup)?;
    health_with(&config, ready, port).await
}

/// [`health`], for a configuration the service already holds, such as one with
/// settings of its own, rather than one read again from the process environment.
///
/// # Errors
///
/// When the server cannot be reached in [`HEALTH_TIMEOUT`], or answers other than
/// `2xx`.
pub async fn health_with(config: &Config, ready: bool, port: Option<u16>) -> Result<()> {
    let port = port
        .or(config.server.internal_port)
        .unwrap_or(config.server.port);
    let path = if ready {
        crate::health::READY
    } else {
        crate::health::LIVE
    };
    probe(&config.server.binding, port, path, HEALTH_TIMEOUT).await?;
    println!("{path}: ok");
    Ok(())
}

/// Ask `GET path` of the server at `binding` and `port`, and succeed on a `2xx`. A
/// binding to every address is asked on loopback.
///
/// # Errors
///
/// Fails when the server cannot be reached within `timeout`, or answers other than
/// `2xx`.
pub async fn probe(binding: &str, port: u16, path: &str, timeout: Duration) -> Result<()> {
    let host = match binding.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST).to_string(),
        Ok(IpAddr::V6(ip)) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST).to_string(),
        _ => binding.to_string(),
    };
    let ask = async {
        let mut stream = TcpStream::connect((host.as_str(), port)).await?;
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
        let mut head = [0_u8; 64];
        let mut read = 0;
        while read < 12 {
            let more = stream.read(&mut head[read..]).await?;
            if more == 0 {
                break;
            }
            read = read.checked_add(more).unwrap_or(head.len()); // ARI-5
        }
        Ok::<_, std::io::Error>(head[..read].to_vec())
    };
    let head = tokio::time::timeout(timeout, ask)
        .await
        .map_err(|_| Error::string(&format!("{path}: no answer from port {port} in time")))?
        .map_err(|error| Error::string(&format!("{path}: port {port}: {error}")))?;
    let status = std::str::from_utf8(&head)
        .ok()
        .and_then(|text| {
            text.strip_prefix("HTTP/1.1 ")
                .or_else(|| text.strip_prefix("HTTP/1.0 "))
        })
        .and_then(|rest| rest.get(..3))
        .and_then(|code| code.parse::<u16>().ok());
    match status {
        Some(code) if (200..300).contains(&code) => Ok(()),
        Some(code) => Err(Error::string(&format!("{path}: answered {code}"))),
        None => Err(Error::string(&format!("{path}: not an HTTP answer"))),
    }
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
    fn reading_the_process_arguments_agrees_between_parse_paths() {
        // The test harness's own arguments decide the outcome; both paths must agree.
        let read = read(Cli::command()).is_ok();
        assert_eq!(Cli::try_parse().is_ok(), read);
    }

    #[tokio::test]
    async fn health_from_refuses_a_closed_port_and_a_refused_configuration() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = health_from(
            &Environment::Test,
            &Defaults::default(),
            true,
            Some(port),
            |name| (name == "SERVER_BINDING").then(|| "127.0.0.1".to_string()),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(&port.to_string()), "{error}");
        assert!(
            health_from(
                &Environment::Test,
                &Defaults::default(),
                false,
                None,
                |name| { (name == "SERVER_PORT").then(|| "not a port".to_string()) }
            )
            .await
            .is_err()
        );
    }

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
    fn health_takes_ready_and_a_port() {
        let Command::Health { ready, port } = parse(&["svc", "health"]).command else {
            panic!("expected health");
        };
        assert!(!ready);
        assert_eq!(port, None);
        let Command::Health { ready, port } =
            parse(&["svc", "health", "--ready", "--port", "9000"]).command
        else {
            panic!("expected health");
        };
        assert!(ready);
        assert_eq!(port, Some(9000));
    }

    /// A server on a local port that answers each connection with `answer`.
    async fn answering(answer: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut asked = vec![0_u8; 1024];
            let read = stream.read(&mut asked).await.unwrap();
            stream.write_all(answer.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&asked[..read]).to_string()
        });
        (port, server)
    }

    #[tokio::test]
    async fn health_passes_on_a_2xx_and_asks_the_path() {
        let (port, server) = answering("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
        probe("0.0.0.0", port, crate::health::READY, HEALTH_TIMEOUT)
            .await
            .unwrap();
        let asked = server.await.unwrap();
        assert!(
            asked.starts_with("GET /_health/ready HTTP/1.1\r\n"),
            "{asked}"
        );
    }

    #[tokio::test]
    async fn health_fails_on_any_other_answer_or_none() {
        let (port, _server) = answering("HTTP/1.1 503 Service Unavailable\r\n\r\n").await;
        let error = probe("127.0.0.1", port, crate::health::READY, HEALTH_TIMEOUT)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("answered 503"), "{error}");
        let (port, _server) = answering("garbage").await;
        assert!(
            probe("127.0.0.1", port, crate::health::LIVE, HEALTH_TIMEOUT)
                .await
                .is_err()
        );
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        assert!(
            probe("127.0.0.1", port, crate::health::LIVE, HEALTH_TIMEOUT)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn health_gives_up_on_a_server_that_does_not_answer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let error = probe(
            "127.0.0.1",
            port,
            crate::health::LIVE,
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("in time"), "{error}");
        drop(listener);
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
            &[("DB_URL", "postgres://localhost/db"), ("PORT", "8000")],
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
                    "DB_URL",
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
        assert!(error.to_string().contains("DB_URL"));
        let pairs = [("DB_URL", "postgres://localhost/db"), ("HTTP_HSTS", "tru")];
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

    #[test]
    fn reading_the_command_line_refuses_as_the_guard_says_and_lets_the_rest_through() {
        let read = |args: &[&str], process: Environment, confirmed: Option<&str>| {
            read_from(
                command("svc", "1.0.0", "A service."),
                args,
                &process,
                confirmed,
            )
        };
        let refused = read(
            &["svc", "db", "reset"],
            Environment::Production,
            Some("true"),
        );
        let Err(Stop::Refused(message)) = refused else {
            panic!("{refused:?}");
        };
        assert!(message.contains(DESTROY_DATA_VAR), "{message}");
        assert!(matches!(
            read(&["svc", "db", "truncate"], Environment::Test, None),
            Err(Stop::Refused(_))
        ));
        assert!(matches!(
            read(
                &["svc", "--environment", "production", "db", "down"],
                Environment::Development,
                Some("true")
            ),
            Err(Stop::Refused(_))
        ));
        let cli = read(&["svc", "db", "reset"], Environment::Test, Some("true")).unwrap();
        assert!(matches!(
            cli.command,
            Command::Db {
                command: DbCommand::Reset
            }
        ));
        let cli = read(&["svc", "start"], Environment::Production, None).unwrap();
        assert!(matches!(cli.command, Command::Start { .. }));

        let Err(Stop::Clap(error)) = read(&["svc", "nope"], Environment::Test, None) else {
            panic!("a bad argument is clap's to answer");
        };
        assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
        let Err(Stop::Clap(version)) = read(&["svc", "--version"], Environment::Test, None) else {
            panic!("--version is clap's to answer");
        };
        assert_eq!(version.kind(), ErrorKind::DisplayVersion);
        assert_eq!(version.to_string(), "svc 1.0.0\n");
    }

    #[tokio::test]
    async fn the_health_command_asks_the_port_it_is_given() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let (port, server) = answering("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
        health(&Environment::Test, &defaults, true, Some(port))
            .await
            .unwrap();
        let asked = server.await.unwrap();
        assert!(asked.starts_with("GET /_health/ready "), "{asked}");
        let (port, server) = answering("HTTP/1.1 204 No Content\r\n\r\n").await;
        health(&Environment::Test, &defaults, false, Some(port))
            .await
            .unwrap();
        let asked = server.await.unwrap();
        assert!(asked.starts_with("GET /_health/live "), "{asked}");
    }

    #[tokio::test]
    async fn health_with_asks_the_configured_internal_port_or_the_one_given() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let (internal, server) = answering("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
        let internal_port = internal.to_string();
        let config = Config::from_lookup(&Environment::Test, &defaults, |name| {
            (name == "INTERNAL_PORT").then(|| internal_port.clone())
        })
        .unwrap();
        health_with(&config, false, None).await.unwrap();
        assert!(server.await.unwrap().starts_with("GET /_health/live "));
        let (given, server) = answering("HTTP/1.1 204 No Content\r\n\r\n").await;
        health_with(&config, true, Some(given)).await.unwrap();
        assert!(server.await.unwrap().starts_with("GET /_health/ready "));
    }

    #[test]
    fn check_with_reads_its_settings_through_the_lookup() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let asked = std::cell::RefCell::new(Vec::new());
        check_with(&Environment::Test, &defaults, |name| {
            asked.borrow_mut().push(name.to_string());
            None
        })
        .unwrap();
        let asked = asked.into_inner();
        for name in ["PORT", "BINDING", "DB_URL"] {
            assert!(
                asked.iter().any(|asked| asked == name),
                "{name} in {asked:?}"
            );
        }
    }
}
