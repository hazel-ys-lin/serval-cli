//! `serval` CLI library entry point.
//!
//! The `serval` binary in `src/bin/serval.rs` is a thin wrapper over
//! [`run`]. Keeping the parser + dispatcher here means tests can drive
//! the CLI without forking a process.
//!
//! Public surface (v0.x, still growing):
//! - `serval status [--server URL] [--json]` — hit `/health`
//! - `serval run <path> --base-url URL [--endpoint P] [--method M] [--json]`
//!   — execute a `.feature` file against an HTTP target
//! - `serval history [--limit N] [--report-dir DIR] [--json]` — list
//!   past run reports under `.serval/reports/`
//! - `serval diff <before-id> <after-id> [--report-dir DIR] [--json]`
//!   — compare two run reports
//! - `serval api {list, show <pattern>, find <query>} [--dir DIR]
//!   [--json]` — inspect `.feature` specs with API frontmatter
//!
//! Exit codes are documented in [`exit`].

pub mod commands;
pub mod exit;
pub mod output;

use std::ffi::OsString;

use clap::{Parser, Subcommand};

use crate::cli::output::OutputFormat;

/// `serval` — spec-anchored API verification CLI.
///
/// More subcommands land as Phase 1 progresses (`run`, `mock`,
/// `history`, `diff`, ...). For now the CLI exists primarily to
/// validate the scaffold.
#[derive(Debug, Parser)]
#[command(name = "serval", version, about, long_about = None)]
struct Cli {
    /// Emit JSON instead of the human-friendly table.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show the upstream server's health report.
    Status(commands::status::StatusArgs),

    /// Execute a `.feature` file against an HTTP target.
    Run(commands::run::RunArgs),

    /// List past run reports under `.serval/reports/`.
    History(commands::history::HistoryArgs),

    /// Compare two run reports.
    Diff(commands::diff::DiffArgs),

    /// Inspect `.feature` specs on disk (list / show / find).
    Api(commands::api::ApiArgs),
}

/// Parse the given argv and run the matching subcommand.
///
/// Returns the CLI's exit code; the binary in `src/bin/serval.rs`
/// passes this straight to `std::process::exit`.
pub fn run<I, T>(argv: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(argv) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };

    let format = if cli.json {
        OutputFormat::Json
    } else {
        OutputFormat::Table
    };

    match cli.command {
        Command::Status(args) => commands::status::run(args, format),
        Command::Run(args) => commands::run::run(args, format),
        Command::History(args) => commands::history::run(args, format),
        Command::Diff(args) => commands::diff::run(args, format),
        Command::Api(args) => commands::api::run(args, format),
    }
}
