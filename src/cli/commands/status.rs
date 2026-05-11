//! `serval status` — hit a server's `/health` endpoint and print its
//! response.
//!
//! Placeholder scaffold today: there's no upstream server yet. Phase 3
//! (`serval mock`) and Phase 4 (agent eval harness) will define
//! concrete `/health` shapes that this command can target.
//!
//! Exit codes:
//! - `0` — server responded `status: ok`.
//! - `1` — server responded with any non-`ok` status.
//! - `2` — couldn't reach the server, or its response wasn't parseable.

use clap::Args;
use serde::{Deserialize, Serialize};

use crate::cli::exit;
use crate::cli::output::{self, OutputFormat};

/// Default server URL when neither `--server` nor `SERVAL_SERVER` env
/// var is set. Placeholder default until Phase 3 mock server lands.
const DEFAULT_SERVER: &str = "http://localhost:3000";

/// `serval status` arguments.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Server base URL. Falls back to `$SERVAL_SERVER`, then to
    /// http://localhost:3000.
    #[arg(long, env = "SERVAL_SERVER")]
    pub server: Option<String>,

    /// HTTP request timeout in seconds.
    #[arg(long, default_value_t = 5)]
    pub timeout: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

pub fn run(args: StatusArgs, format: OutputFormat) -> i32 {
    let server = args.server.unwrap_or_else(|| DEFAULT_SERVER.to_string());
    let url = format!("{}/health", server.trim_end_matches('/'));

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(args.timeout))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to build HTTP client: {e}");
            return exit::SYSTEM_ERROR;
        }
    };

    let resp = match client.get(&url).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: could not reach {url}: {e}");
            return exit::SYSTEM_ERROR;
        }
    };

    let health: HealthResponse = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: server at {url} returned a body we could not parse: {e}");
            return exit::SYSTEM_ERROR;
        }
    };

    let overall = health.status.clone();
    output::print(format, Some(&format!("Server: {server}")), &health);

    if overall == "ok" {
        exit::OK
    } else {
        exit::TEST_FAILED
    }
}
