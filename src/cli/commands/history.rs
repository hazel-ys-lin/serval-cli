//! `serval history` — list past JSON reports under
//! `.serval/reports/` (or `--report-dir`), newest first.

use std::path::PathBuf;

use clap::Args;
use serde::Serialize;

use crate::cli::exit;
use crate::cli::output::OutputFormat;
use crate::error::Error;
use crate::report::{self, ReportRecord};

/// `serval history` arguments.
#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Directory holding the JSON reports. Falls back to
    /// `$SERVAL_REPORT_DIR`, then `.serval/reports`.
    #[arg(long, env = "SERVAL_REPORT_DIR")]
    pub report_dir: Option<PathBuf>,

    /// Maximum number of reports to show (newest first).
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

pub fn run(args: HistoryArgs, format: OutputFormat) -> i32 {
    let dir = args
        .report_dir
        .unwrap_or_else(|| PathBuf::from(".serval").join("reports"));

    match report::list(&dir) {
        Ok(mut records) => {
            records.truncate(args.limit);
            print_history(&records, format);
            exit::OK
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

fn print_history(records: &[ReportRecord], format: OutputFormat) {
    match format {
        OutputFormat::Json => print_json(records),
        OutputFormat::Table => print_table(records),
    }
}

fn print_table(records: &[ReportRecord]) {
    if records.is_empty() {
        println!("  no reports found");
        return;
    }
    println!(
        "  {:<32}  {:<19}  {:<10}  SOURCE",
        "ID", "STARTED", "SUMMARY"
    );
    for r in records {
        let started = format_started(r.report.started_at);
        let summary = format!(
            "{}/{} {}",
            r.report.summary.passed,
            r.report.summary.total,
            if r.report.summary.failed > 0 {
                "(FAIL)"
            } else {
                "(OK)"
            }
        );
        let source = std::path::Path::new(&r.report.source_file)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| r.report.source_file.clone());
        println!(
            "  {:<32}  {:<19}  {:<10}  {}",
            r.id, started, summary, source
        );
    }
}

fn format_started(t: time::OffsetDateTime) -> String {
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    t.format(fmt)
        .unwrap_or_else(|_| t.unix_timestamp().to_string())
}

#[derive(Serialize)]
struct HistoryEntry<'a> {
    id: &'a str,
    #[serde(with = "time::serde::rfc3339")]
    started_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    finished_at: time::OffsetDateTime,
    source_file: &'a str,
    target: &'a crate::report::TargetRef,
    summary: &'a crate::report::RunSummary,
}

fn print_json(records: &[ReportRecord]) {
    let view: Vec<HistoryEntry> = records
        .iter()
        .map(|r| HistoryEntry {
            id: &r.id,
            started_at: r.report.started_at,
            finished_at: r.report.finished_at,
            source_file: &r.report.source_file,
            target: &r.report.target,
            summary: &r.report.summary,
        })
        .collect();
    match serde_json::to_string_pretty(&view) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("internal: serialize history: {e}"),
    }
}
