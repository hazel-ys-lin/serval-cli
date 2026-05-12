//! `serval diff <before> <after>` — compare two JSON reports and
//! list which scenarios flipped, were added, or were removed.

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Args;
use serde::Serialize;

use crate::cli::exit;
use crate::cli::output::OutputFormat;
use crate::error::Error;
use crate::report::{self, ReportRecord, RunSummary, TargetRef};
use crate::runner::TestResult;

/// `serval diff` arguments.
#[derive(Debug, Args)]
pub struct DiffArgs {
    /// ID of the earlier report (filename without `.json`, a unique
    /// prefix of one, or the keywords `latest` / `previous`).
    pub before: String,

    /// ID of the later report (same conventions as `before`).
    pub after: String,

    /// Directory holding the JSON reports. Falls back to
    /// `$SERVAL_REPORT_DIR`, then `.serval/reports`.
    #[arg(long, env = "SERVAL_REPORT_DIR")]
    pub report_dir: Option<PathBuf>,
}

pub fn run(args: DiffArgs, format: OutputFormat) -> i32 {
    let dir = args
        .report_dir
        .unwrap_or_else(|| PathBuf::from(".serval").join("reports"));

    let before = match report::resolve(&dir, &args.before) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };
    let after = match report::resolve(&dir, &args.after) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };

    let diff = compute_diff(&before, &after);
    print_diff(&diff, format);
    exit::OK
}

fn map_error_to_exit(e: &Error) -> i32 {
    match e {
        Error::Spec(_) => exit::SPEC_ERROR,
        Error::System(_) | Error::Io(_) | Error::Http(_) => exit::SYSTEM_ERROR,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportDiff {
    pub before_id: String,
    pub after_id: String,
    pub source_changed: bool,
    pub target_changed: bool,
    pub scenarios: Vec<ScenarioChange>,
    pub summary_before: RunSummary,
    pub summary_after: RunSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum ScenarioChange {
    /// Scenario present in both reports but pass status flipped.
    Flipped {
        title: String,
        example_index: i32,
        /// Whether the scenario passed in the `before` report.
        was_pass: bool,
        /// Whether the scenario passes in the `after` report.
        now_pass: bool,
        status_before: i16,
        status_after: i16,
    },
    /// Scenario only in the `after` report.
    Added {
        title: String,
        example_index: i32,
        pass: bool,
    },
    /// Scenario only in the `before` report.
    Removed {
        title: String,
        example_index: i32,
        pass: bool,
    },
}

fn compute_diff(before: &ReportRecord, after: &ReportRecord) -> ReportDiff {
    let before_map = index_by_scenario(&before.report.results);
    let after_map = index_by_scenario(&after.report.results);

    let mut changes = Vec::new();

    for (key, b) in &before_map {
        match after_map.get(key) {
            Some(a) => {
                if b.pass != a.pass {
                    changes.push(ScenarioChange::Flipped {
                        title: key.0.clone(),
                        example_index: key.1,
                        was_pass: b.pass,
                        now_pass: a.pass,
                        status_before: b.response_status,
                        status_after: a.response_status,
                    });
                }
            }
            None => changes.push(ScenarioChange::Removed {
                title: key.0.clone(),
                example_index: key.1,
                pass: b.pass,
            }),
        }
    }

    for (key, a) in &after_map {
        if !before_map.contains_key(key) {
            changes.push(ScenarioChange::Added {
                title: key.0.clone(),
                example_index: key.1,
                pass: a.pass,
            });
        }
    }

    ReportDiff {
        before_id: before.id.clone(),
        after_id: after.id.clone(),
        source_changed: before.report.source_file != after.report.source_file,
        target_changed: !target_equal(&before.report.target, &after.report.target),
        scenarios: changes,
        summary_before: before.report.summary.clone(),
        summary_after: after.report.summary.clone(),
    }
}

fn index_by_scenario(results: &[TestResult]) -> HashMap<(String, i32), &TestResult> {
    results
        .iter()
        .map(|r| ((r.scenario_title.clone(), r.example_index), r))
        .collect()
}

fn target_equal(a: &TargetRef, b: &TargetRef) -> bool {
    a.base_url == b.base_url && a.endpoint == b.endpoint && a.method == b.method
}

fn print_diff(diff: &ReportDiff, format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(diff) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("internal: serialize diff: {e}"),
        },
        OutputFormat::Table => print_diff_table(diff),
    }
}

fn print_diff_table(diff: &ReportDiff) {
    println!("  comparing {} -> {}", diff.before_id, diff.after_id);
    if diff.source_changed {
        println!("  (!) source_file changed");
    }
    if diff.target_changed {
        println!("  (!) target changed");
    }
    println!();

    if diff.scenarios.is_empty() {
        println!("  CHANGES: none");
    } else {
        println!("  CHANGES");
        for ch in &diff.scenarios {
            match ch {
                ScenarioChange::Flipped {
                    title,
                    was_pass,
                    now_pass,
                    status_before,
                    status_after,
                    ..
                } => {
                    let arrow = format!(
                        "{} -> {}",
                        if *was_pass { "PASS" } else { "FAIL" },
                        if *now_pass { "PASS" } else { "FAIL" }
                    );
                    println!("    [{arrow}] {title}  ({status_before} -> {status_after})");
                }
                ScenarioChange::Added { title, pass, .. } => {
                    let mark = if *pass { "PASS" } else { "FAIL" };
                    println!("    [+] {title}  ({mark})");
                }
                ScenarioChange::Removed { title, pass, .. } => {
                    let mark = if *pass { "PASS" } else { "FAIL" };
                    println!("    [-] {title}  (was {mark})");
                }
            }
        }
    }

    println!();
    println!("  SUMMARY");
    println!(
        "    before: {} passed, {} failed ({} total)",
        diff.summary_before.passed, diff.summary_before.failed, diff.summary_before.total
    );
    println!(
        "    after:  {} passed, {} failed ({} total)",
        diff.summary_after.passed, diff.summary_after.failed, diff.summary_after.total
    );
}
