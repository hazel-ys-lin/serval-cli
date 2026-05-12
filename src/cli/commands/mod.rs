//! Subcommand implementations for the `serval` CLI.
//!
//! Each module owns one subcommand: argument parsing (its own `Args`
//! struct used by clap), the work it performs, and the exit code it
//! decides on. Subcommands are wired into the top-level dispatcher in
//! [`crate::cli`].

pub mod run;
pub mod status;
