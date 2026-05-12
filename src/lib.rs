//! `serval-cli` library crate.
//!
//! Exposes the CLI surface (`cli::run`) so integration tests can drive
//! it without forking a process. The `serval` binary in
//! `src/bin/serval.rs` is a thin wrapper over `cli::run`.
//!
//! Core building blocks live as sibling modules so subcommands and
//! external callers can compose them:
//! - [`error`] — `Error` enum + `Result` alias mapped to CLI exit codes
//! - [`config`] — `~/.serval/config.toml` reader / writer + named
//!   environment resolution
//! - [`frontmatter`] — optional YAML frontmatter parser for `.feature`
//!   files (`api.path` / `api.method` / `collection` / `implements`)
//! - [`gherkin`] — `.feature` file parser → `ParsedFeature` DTOs
//! - [`patterns`] — step-pattern engine mapping step text to runner
//!   actions (Phase 2.1+; built-in table only for now)
//! - [`spec`] — permissive `.feature` file loader (multi-Feature,
//!   stale language directives)
//! - [`runner`] — async HTTP test runner consuming parsed scenarios
//! - [`report`] — on-disk JSON run report written under
//!   `.serval/reports/` after each `serval run`

pub mod cli;
pub mod config;
pub mod error;
pub mod frontmatter;
pub mod gherkin;
pub mod patterns;
pub mod report;
pub mod runner;
pub mod spec;
