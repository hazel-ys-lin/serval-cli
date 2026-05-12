//! Error and Result types for the `serval-cli` library crate.
//!
//! Top-level variants signal the broad category — `Spec` for user-input
//! problems, `System` / `Io` / `Http` for infrastructure failures. The
//! CLI wires these to its exit-code contract (see `cli::exit`):
//! `Spec` maps to exit 3 (`SPEC_ERROR`), the rest to exit 2
//! (`SYSTEM_ERROR`).
//!
//! Test-assertion failures (e.g. "expected 200, got 500") are **not**
//! errors: they come back as `pass: false` on a `TestResult` and the
//! CLI returns exit 1 (`TEST_FAILED`).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// Bad input: malformed Gherkin, invalid URL, unsupported HTTP
    /// method, missing argument.
    #[error("spec error: {0}")]
    Spec(String),

    /// Infrastructure failure: HTTP client build failure, network
    /// timeout, server unreachable, JSON serialization issue.
    #[error("system error: {0}")]
    System(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
