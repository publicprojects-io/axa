#![allow(
    clippy::unwrap_used,
    reason = "RCA-SCOPE-4: tests may unwrap, expect and index"
)]
//! `axa::cli::parse` and `Cli::parse()` as a process meets them: they exit on a bad
//! argument, on `--help` and `--version`, and, with `1`, on a command that destroys
//! data where that is refused. A function that exits cannot be called in a test's own
//! process, so this test starts itself again with the arguments it wants read.
//!
//! It has no test harness (`harness = false`), so the child's arguments are exactly
//! the ones given here, and none of the harness's.

use std::process::{Command, Output};

use axa::cli::{self, Cli};
use clap::Parser;

/// Set in the child, naming which reader it calls.
const ROLE: &str = "AXA_CLI_EXIT_TEST_ROLE";

fn main() {
    match std::env::var(ROLE).as_deref() {
        Ok("parse") => {
            let cli = cli::parse("svc", "9.9.9", "A test service.");
            println!("read {:?}", cli.command);
        }
        Ok("trait") => {
            let cli = Cli::parse();
            println!("read {:?}", cli.command);
        }
        _ => parent(),
    }
}

/// Run this binary as the child with `role`, `args`, and the process environment `app_env`.
fn child(role: &str, args: &[&str], app_env: &str) -> Output {
    run(role, args, app_env, None)
}

/// [`child`], with `cli::DESTROY_DATA_VAR` set to `confirmed` when given.
fn run(role: &str, args: &[&str], app_env: &str, confirmed: Option<&str>) -> Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(args)
        .env(ROLE, role)
        .env("APP_ENV", app_env)
        .env_remove(cli::DESTROY_DATA_VAR);
    if let Some(confirmed) = confirmed {
        command.env(cli::DESTROY_DATA_VAR, confirmed);
    }
    command.output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn parent() {
    // A service's own name and version answer `--version`, and the process exits 0.
    let output = child("parse", &["--version"], "development");
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("svc 9.9.9"));

    let output = child("parse", &["--help"], "development");
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("A test service."));

    // A bad argument exits as clap does, with 2.
    let output = child("parse", &["--no-such-flag"], "development");
    assert_eq!(output.status.code(), Some(2));
    let output = child("trait", &["--no-such-flag"], "development");
    assert_eq!(output.status.code(), Some(2));

    // A command that destroys data is refused, with 1 and before anything runs, in
    // production even when confirmed, and anywhere unconfirmed.
    for role in ["parse", "trait"] {
        for (app_env, confirmed) in [
            ("production", Some("true")),
            ("production", None),
            ("development", None),
        ] {
            let output = run(role, &["db", "reset"], app_env, confirmed);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{role} {app_env} {confirmed:?}"
            );
            assert!(text(&output.stderr).contains("refused"), "{role}");
            assert!(!text(&output.stdout).contains("read"), "{role}");
        }
    }

    // It reads where it is allowed, and a harmless command reads in production.
    for role in ["parse", "trait"] {
        for (args, app_env, confirmed) in [
            (["db", "reset"], "development", Some("true")),
            (["db", "status"], "production", None),
        ] {
            let output = run(role, &args, app_env, confirmed);
            assert_eq!(output.status.code(), Some(0), "{role} {args:?} {app_env}");
            assert!(text(&output.stdout).contains("read"), "{role}");
        }
    }
    println!("cli_exit: ok");
}
