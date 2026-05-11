//! `serval-cli` library crate.
//!
//! Exposes the CLI surface (`cli::run`) so integration tests can drive
//! it without forking a process. The `serval` binary in
//! `src/bin/serval.rs` is a thin wrapper over `cli::run`.

pub mod cli;
