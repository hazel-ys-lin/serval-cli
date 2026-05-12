//! `serval spec validate` — parse-check `.feature` files. No HTTP,
//! no runner; just frontmatter + Gherkin parsing. Useful as a
//! pre-commit / CI gate that catches malformed specs before they
//! reach `serval run`.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::exit;
use crate::cli::output::OutputFormat;
use crate::error::{Error, Result};
use crate::frontmatter;
use crate::spec;

#[derive(Debug, Args)]
pub struct SpecArgs {
    #[command(subcommand)]
    pub action: SpecAction,
}

#[derive(Debug, Subcommand)]
pub enum SpecAction {
    /// Parse-check `.feature` files. Exits 3 if any fail.
    Validate(ValidateArgs),
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Path to a `.feature` file or a directory containing them
    /// (walked recursively). Default: `specs/` in the cwd.
    #[arg(default_value = "specs")]
    pub path: PathBuf,
}

pub fn run(args: SpecArgs, format: OutputFormat) -> i32 {
    match args.action {
        SpecAction::Validate(a) => run_validate(a, format),
    }
}

fn run_validate(args: ValidateArgs, format: OutputFormat) -> i32 {
    let targets = match collect_targets(&args.path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };

    let results: Vec<ValidationResult> = targets.iter().map(|p| validate_one(p)).collect();
    print_results(&results, format);

    if results.iter().any(|r| r.error.is_some()) {
        exit::SPEC_ERROR
    } else {
        exit::OK
    }
}

fn collect_targets(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        Ok(vec![path.to_path_buf()])
    } else if path.is_dir() {
        Ok(spec::collect_feature_paths(path))
    } else {
        Err(Error::Spec(format!(
            "path does not exist: {}",
            path.display()
        )))
    }
}

fn map_error_to_exit(e: &Error) -> i32 {
    match e {
        Error::Spec(_) => exit::SPEC_ERROR,
        Error::System(_) | Error::Io(_) | Error::Http(_) => exit::SYSTEM_ERROR,
    }
}

#[derive(Debug, Serialize)]
struct ValidationResult {
    path: String,
    status: Status,
    features: Option<usize>,
    scenarios: Option<usize>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Ok,
    Error,
}

fn validate_one(path: &Path) -> ValidationResult {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => {
            return ValidationResult {
                path: path.to_string_lossy().into_owned(),
                status: Status::Error,
                features: None,
                scenarios: None,
                error: Some(format!("read error: {e}")),
            };
        }
    };

    let body = match frontmatter::split(&raw) {
        Ok((_, body)) => body,
        Err(e) => {
            return ValidationResult {
                path: path.to_string_lossy().into_owned(),
                status: Status::Error,
                features: None,
                scenarios: None,
                error: Some(e.to_string()),
            };
        }
    };

    match spec::parse_relaxed(body) {
        Ok(features) => {
            let scenarios = features.iter().map(|f| f.scenarios.len()).sum();
            ValidationResult {
                path: path.to_string_lossy().into_owned(),
                status: Status::Ok,
                features: Some(features.len()),
                scenarios: Some(scenarios),
                error: None,
            }
        }
        Err(e) => ValidationResult {
            path: path.to_string_lossy().into_owned(),
            status: Status::Error,
            features: None,
            scenarios: None,
            error: Some(e.to_string()),
        },
    }
}

fn print_results(results: &[ValidationResult], format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(results) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("internal: serialize validate output: {e}"),
        },
        OutputFormat::Table => print_table(results),
    }
}

fn print_table(results: &[ValidationResult]) {
    if results.is_empty() {
        println!("  no `.feature` files found");
        return;
    }
    println!(
        "  {:<6}  {:<8}  {:<10}  PATH",
        "STATUS", "FEATURES", "SCENARIOS"
    );
    for r in results {
        let status = match r.status {
            Status::Ok => "OK",
            Status::Error => "ERR",
        };
        let features = r
            .features
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        let scenarios = r
            .scenarios
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!("  {status:<6}  {features:<8}  {scenarios:<10}  {}", r.path);
        if let Some(err) = &r.error {
            println!("          {err}");
        }
    }
    println!();
    let total = results.len();
    let valid = results.iter().filter(|r| r.error.is_none()).count();
    let invalid = total - valid;
    println!("  {total} file(s) scanned: {valid} valid, {invalid} invalid.");
}
