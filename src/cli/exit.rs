//! Exit-code contract for the `serval` CLI.
//!
//! Stable across all subcommands. Scripts and CI rely on these values, so
//! treat them as part of the public CLI API.
//!
//! - `0` — operation completed and any test assertions passed.
//! - `1` — operation completed but a test or spec assertion **failed**
//!   (e.g. `serval run` had failing scenarios). Not an error.
//! - `2` — system / infrastructure error (network, auth, server down,
//!   IO). The CLI couldn't complete the operation.
//! - `3` — bad spec / bad input (invalid URL, malformed Gherkin,
//!   unsupported scheme, missing argument). The user needs to fix
//!   their input.
//!
//! Returned from `cli::run`; the binary entry point passes it straight
//! to `std::process::exit`.

pub const OK: i32 = 0;
pub const TEST_FAILED: i32 = 1;
pub const SYSTEM_ERROR: i32 = 2;
pub const SPEC_ERROR: i32 = 3;
