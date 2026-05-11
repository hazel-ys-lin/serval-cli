//! Entry point for the `serval` CLI.
//!
//! Thin wrapper around `serval_cli::cli::run`; all logic lives in the
//! library so the same surface can be exercised from integration tests
//! without re-implementing argument parsing.

fn main() {
    let exit_code = serval_cli::cli::run(std::env::args_os());
    std::process::exit(exit_code);
}
