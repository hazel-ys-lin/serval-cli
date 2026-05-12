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

/// A report on disk plus the id used to reference it (the filename
/// with `.json` stripped).
#[derive(Debug, Clone)]
pub struct ReportRecord {
    pub id: String,
    pub report: RunReport,
}

/// Read a single JSON report file into a [`RunReport`].
pub fn read(path: &Path) -> Result<RunReport> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::System(format!("read report {}: {e}", path.display())))?;
    serde_json::from_str(&content)
        .map_err(|e| Error::Spec(format!("malformed report {}: {e}", path.display())))
}

/// List every `*.json` report under `dir` that deserializes
/// successfully, sorted by `started_at` descending (newest first).
/// Missing directory returns an empty list rather than an error.
pub fn list(dir: &Path) -> Result<Vec<ReportRecord>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let read_dir = std::fs::read_dir(dir)
        .map_err(|e| Error::System(format!("read dir {}: {e}", dir.display())))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| Error::System(format!("read dir entry: {e}")))?;
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(report) = read(&path) else { continue };
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(ReportRecord { id, report });
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.report.started_at));
    Ok(out)
}

/// Resolve a user-supplied id to a single [`ReportRecord`]. Accepts:
/// - exact filename without `.json` extension
/// - unique prefix of an existing filename
/// - keyword `latest` (newest report by `started_at`)
/// - keyword `previous` (second-newest)
pub fn resolve(dir: &Path, id: &str) -> Result<ReportRecord> {
    if id == "latest" || id == "previous" {
        let mut records = list(dir)?;
        let index = if id == "latest" { 0 } else { 1 };
        if records.len() > index {
            return Ok(records.swap_remove(index));
        }
        return Err(Error::Spec(format!(
            "no report at position {id} (have {} report(s))",
            records.len()
        )));
    }

    let exact = dir.join(format!("{id}.json"));
    if exact.exists() {
        let report = read(&exact)?;
        return Ok(ReportRecord {
            id: id.to_string(),
            report,
        });
    }

    if !dir.exists() {
        return Err(Error::Spec(format!(
            "no reports directory at {}",
            dir.display()
        )));
    }

    let read_dir = std::fs::read_dir(dir)
        .map_err(|e| Error::System(format!("read dir {}: {e}", dir.display())))?;
    let mut matches: Vec<PathBuf> = read_dir
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with(id) && name.ends_with(".json")
        })
        .map(|e| e.path())
        .collect();

    match matches.len() {
        0 => Err(Error::Spec(format!("no report matches id {id:?}"))),
        1 => {
            let path = matches.remove(0);
            let report = read(&path)?;
            let resolved_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(ReportRecord {
                id: resolved_id,
                report,
            })
        }
        n => Err(Error::Spec(format!(
            "ambiguous id {id:?} matches {n} reports"
        ))),
    }
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

    fn report_at(seconds: i64) -> RunReport {
        let mut r = mk_report();
        let t = time::OffsetDateTime::from_unix_timestamp(seconds).unwrap();
        r.started_at = t;
        r.finished_at = t;
        r
    }

    fn write_named(dir: &Path, id: &str, report: &RunReport) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(report).unwrap()).unwrap();
        path
    }

    #[test]
    fn list_returns_empty_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let records = list(&missing).expect("missing dir should be empty, not error");
        assert!(records.is_empty());
    }

    #[test]
    fn list_sorts_by_started_at_descending() {
        let tmp = tempfile::tempdir().unwrap();
        write_named(tmp.path(), "alpha", &report_at(1_000_000_000));
        write_named(tmp.path(), "beta", &report_at(2_000_000_000));
        write_named(tmp.path(), "gamma", &report_at(1_500_000_000));

        let records = list(tmp.path()).expect("list");
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["beta", "gamma", "alpha"]);
    }

    #[test]
    fn list_skips_non_json_and_malformed_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_named(tmp.path(), "good", &report_at(1_000_000_000));
        std::fs::write(tmp.path().join("not-a-report.json"), "{ not json").unwrap();
        std::fs::write(tmp.path().join("readme.txt"), "not even json").unwrap();

        let records = list(tmp.path()).expect("list");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "good");
    }

    #[test]
    fn resolve_exact_id() {
        let tmp = tempfile::tempdir().unwrap();
        write_named(tmp.path(), "abc123", &report_at(1_000_000_000));
        let rec = resolve(tmp.path(), "abc123").expect("resolve");
        assert_eq!(rec.id, "abc123");
    }

    #[test]
    fn resolve_unique_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        write_named(tmp.path(), "2026-05-12T03-30", &report_at(1_000_000_000));
        write_named(tmp.path(), "2026-05-13T11-22", &report_at(2_000_000_000));
        let rec = resolve(tmp.path(), "2026-05-12").expect("unique prefix");
        assert_eq!(rec.id, "2026-05-12T03-30");
    }

    #[test]
    fn resolve_ambiguous_prefix_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write_named(tmp.path(), "2026-05-12T03-30", &report_at(1_000_000_000));
        write_named(tmp.path(), "2026-05-12T11-22", &report_at(2_000_000_000));
        let err = resolve(tmp.path(), "2026-05-12").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "got {msg}");
    }

    #[test]
    fn resolve_latest_and_previous_keywords() {
        let tmp = tempfile::tempdir().unwrap();
        write_named(tmp.path(), "older", &report_at(1_000_000_000));
        write_named(tmp.path(), "newer", &report_at(2_000_000_000));

        let latest = resolve(tmp.path(), "latest").expect("latest");
        assert_eq!(latest.id, "newer");

        let previous = resolve(tmp.path(), "previous").expect("previous");
        assert_eq!(previous.id, "older");
    }

    #[test]
    fn resolve_no_match_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write_named(tmp.path(), "abc", &report_at(1_000_000_000));
        let err = resolve(tmp.path(), "zzz").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no report matches"), "got {msg}");
    }
}
