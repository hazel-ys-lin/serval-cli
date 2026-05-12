//! `serval run` — execute a Gherkin `.feature` file against an HTTP
//! target and report pass/fail per scenario.
//!
//! The file may begin with an optional YAML frontmatter block (see
//! [`crate::frontmatter`]) providing `api.path` and `api.method`.
//! When the frontmatter is absent, the same values must come from
//! `--endpoint` and `--method` flags; missing both fails with exit
//! code 3 (`SPEC_ERROR`).
//!
//! The dispatcher in [`crate::cli`] stays sync — a single-threaded
//! tokio runtime is built locally so the async
//! [`crate::runner::TestRunner::run_scenario`] can be awaited.
//!
//! Exit codes (per the CLI contract in [`crate::cli::exit`]):
//! - `0` — all scenarios passed
//! - `1` — at least one scenario assertion failed
//! - `2` — system / network error
//! - `3` — bad input (missing target, malformed file, etc.)

use std::path::PathBuf;
use std::time::Duration;

use clap::Args;

use crate::cli::exit;
use crate::cli::output::OutputFormat;
use crate::error::{Error, Result};
use crate::frontmatter::{self, Frontmatter};
use crate::runner::{ApiSpec, EnvSpec, TestConfig, TestResult, TestRunner};
use crate::spec;

/// `serval run` arguments.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Path to a `.feature` file.
    pub path: PathBuf,

    /// Base URL of the target server (e.g. `http://localhost:3000`).
    /// Falls back to `$SERVAL_BASE_URL`.
    #[arg(long, env = "SERVAL_BASE_URL")]
    pub base_url: String,

    /// API path on the target server (e.g. `/api/users`). Required
    /// when the `.feature` file has no frontmatter providing
    /// `api.path`.
    #[arg(long)]
    pub endpoint: Option<String>,

    /// HTTP method (GET / POST / PUT / DELETE / PATCH / HEAD /
    /// OPTIONS). Required when the `.feature` file has no
    /// frontmatter providing `api.method`.
    #[arg(long)]
    pub method: Option<String>,

    /// Per-request timeout in seconds.
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,
}

pub fn run(args: RunArgs, format: OutputFormat) -> i32 {
    match execute(args, format) {
        Ok(any_failed) => {
            if any_failed {
                exit::TEST_FAILED
            } else {
                exit::OK
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            map_error_to_exit(&e)
        }
    }
}

fn map_error_to_exit(e: &Error) -> i32 {
    match e {
        Error::Spec(_) => exit::SPEC_ERROR,
        Error::System(_) | Error::Io(_) | Error::Http(_) => exit::SYSTEM_ERROR,
    }
}

fn execute(args: RunArgs, format: OutputFormat) -> Result<bool> {
    let raw = std::fs::read_to_string(&args.path)?;
    let (fm, body) = frontmatter::split(&raw)?;
    let api = resolve_api_spec(&fm, &args)?;
    let env = EnvSpec {
        base_url: args.base_url.clone(),
    };

    let features = spec::parse_relaxed(body)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::System(format!("tokio runtime: {e}")))?;

    let runner = TestRunner::with_config(TestConfig {
        timeout: Duration::from_secs(args.timeout),
        ..Default::default()
    })?;

    let all_results = runtime.block_on(async {
        let mut all = Vec::new();
        for feature in &features {
            for scenario in &feature.scenarios {
                let results = runner.run_scenario(scenario, &api, &env).await?;
                all.extend(results);
            }
        }
        Result::Ok(all)
    })?;

    print_results(&all_results, format);
    Ok(all_results.iter().any(|r| !r.pass))
}

fn resolve_api_spec(fm: &Option<Frontmatter>, args: &RunArgs) -> Result<ApiSpec> {
    let fm_api = fm.as_ref().and_then(|f| f.api.as_ref());

    let endpoint = match (&args.endpoint, fm_api) {
        (Some(e), _) => e.clone(),
        (None, Some(a)) => a.path.clone(),
        (None, None) => {
            return Err(Error::Spec(
                "missing `--endpoint`: no `api.path` in frontmatter and no flag provided"
                    .to_string(),
            ));
        }
    };

    let method = match (&args.method, fm_api) {
        (Some(m), _) => m.clone(),
        (None, Some(a)) => a.method.clone(),
        (None, None) => {
            return Err(Error::Spec(
                "missing `--method`: no `api.method` in frontmatter and no flag provided"
                    .to_string(),
            ));
        }
    };

    Ok(ApiSpec {
        endpoint,
        http_method: method,
    })
}

fn print_results(results: &[TestResult], format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(results) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("internal: failed to serialize JSON output: {e}"),
        },
        OutputFormat::Table => {
            for r in results {
                let mark = if r.pass { "PASS" } else { "FAIL" };
                println!(
                    "  [{mark}] {} (status {}, {}ms)",
                    r.scenario_title, r.response_status, r.request_duration_ms
                );
                if let Some(err) = &r.error_message {
                    println!("         {err}");
                }
            }
            let total = results.len();
            let passed = results.iter().filter(|r| r.pass).count();
            let failed = total - passed;
            println!();
            println!("  {passed} passed, {failed} failed, {total} total");
        }
    }
}
