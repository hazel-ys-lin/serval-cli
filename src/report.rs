//! Per-run JSON report written to disk after `serval run`.
//!
//! Layout (`<dir>/<rfc3339-timestamp>.json`):
//!
//! ```jsonc
//! {
//!   "schema_version": 1,
//!   "started_at": "2026-05-12T03:33:12.123Z",
//!   "finished_at": "2026-05-12T03:33:12.456Z",
//!   "source_file": "/abs/path/to/foo.feature",
//!   "target": { "base_url": "...", "endpoint": "...", "method": "..." },
//!   "summary": { "total": 3, "passed": 2, "failed": 1 },
//!   "results": [ TestResult, ... ]
//! }
//! ```
//!
//! Reports are append-only by design — the filename is the run's
//! start timestamp, so each invocation produces a unique file. Later
//! subcommands (`history`, `diff`) read the directory and compare
//! across runs.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;

use crate::error::{Error, Result};
use crate::runner::TestResult;

/// Bump when the on-disk shape changes in a non-additive way so
/// older `history` / `diff` invocations can detect mismatches.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub schema_version: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub finished_at: time::OffsetDateTime,
    pub source_file: String,
    pub target: TargetRef,
    pub summary: RunSummary,
    pub results: Vec<TestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetRef {
    pub base_url: String,
    pub endpoint: String,
    pub method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
}

impl RunSummary {
    pub fn from_results(results: &[TestResult]) -> Self {
        let passed = results.iter().filter(|r| r.pass).count();
        Self {
            total: results.len(),
            passed,
            failed: results.len() - passed,
        }
    }
}

/// Write `report` as pretty-printed JSON into `dir`, creating the
/// directory if it does not exist. Returns the full path of the
/// written file. The filename is derived from `report.started_at`
/// in RFC 3339 form with colons replaced by dashes for filesystem
/// safety on Windows; if RFC 3339 formatting fails (extremely
/// unusual) the unix timestamp is used instead.
pub fn write(report: &RunReport, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::System(format!("create report dir {}: {e}", dir.display())))?;
    let path = dir.join(filename(report.started_at));
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| Error::System(format!("serialize report: {e}")))?;
    std::fs::write(&path, json)
        .map_err(|e| Error::System(format!("write report {}: {e}", path.display())))?;
    Ok(path)
}

fn filename(t: time::OffsetDateTime) -> String {
    let stamp = t
        .format(&Rfc3339)
        .unwrap_or_else(|_| t.unix_timestamp().to_string());
    format!("{}.json", stamp.replace(':', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_result(pass: bool) -> TestResult {
        TestResult {
            scenario_title: "x".to_string(),
            example_index: 0,
            pass,
            error_message: None,
            response_status: 200,
            response_data: None,
            request_duration_ms: 1,
            request_time: time::OffsetDateTime::now_utc(),
        }
    }

    fn mk_report() -> RunReport {
        let now = time::OffsetDateTime::now_utc();
        RunReport {
            schema_version: SCHEMA_VERSION,
            started_at: now,
            finished_at: now,
            source_file: "/x.feature".to_string(),
            target: TargetRef {
                base_url: "http://localhost".to_string(),
                endpoint: "/".to_string(),
                method: "GET".to_string(),
            },
            summary: RunSummary {
                total: 0,
                passed: 0,
                failed: 0,
            },
            results: vec![],
        }
    }

    #[test]
    fn filename_is_filesystem_safe_and_ends_with_json() {
        let t = time::OffsetDateTime::from_unix_timestamp(1_747_010_192).unwrap();
        let name = filename(t);
        assert!(!name.contains(':'), "no colon allowed in filename: {name}");
        assert!(name.ends_with(".json"));
    }

    #[test]
    fn summary_counts_pass_and_fail() {
        let results = vec![mk_result(true), mk_result(true), mk_result(false)];
        let s = RunSummary::from_results(&results);
        assert_eq!(s.total, 3);
        assert_eq!(s.passed, 2);
        assert_eq!(s.failed, 1);
    }

    #[test]
    fn write_creates_directory_and_round_trips_through_json() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("reports");
        let report = mk_report();

        let path = write(&report, &nested).expect("write must succeed");
        assert!(path.exists(), "report file must exist");

        let json = std::fs::read_to_string(&path).unwrap();
        let parsed: RunReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert_eq!(parsed.target.endpoint, "/");
    }
}
