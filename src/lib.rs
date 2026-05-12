//! `serval-cli` library crate.
//!
//! Exposes the CLI surface (`cli::run`) so integration tests can drive
//! it without forking a process. The `serval` binary in
//! `src/bin/serval.rs` is a thin wrapper over `cli::run`.
//!
//! Core building blocks live as sibling modules so subcommands and
//! external callers can compose them:
//! - [`error`] — `Error` enum + `Result` alias mapped to CLI exit codes
//! - [`gherkin`] — `.feature` file parser → `ParsedFeature` DTOs
//! - [`spec`] — permissive `.feature` file loader (multi-Feature,
//!   stale language directives)
//! - [`runner`] — async HTTP test runner consuming parsed scenarios

pub mod cli;
pub mod error;
pub mod gherkin;
pub mod runner;
pub mod spec;
