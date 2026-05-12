//! Subcommand implementations for the `serval` CLI.
//!
//! Each module owns one subcommand: argument parsing (its own `Args`
//! struct used by clap), the work it performs, and the exit code it
//! decides on. Subcommands are wired into the top-level dispatcher in
//! [`crate::cli`].

pub mod api;
pub mod config;
pub mod diff;
pub mod env;
pub mod history;
pub mod run;
pub mod spec;
pub mod status;
