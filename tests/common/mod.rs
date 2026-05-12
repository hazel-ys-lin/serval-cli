//! Shared test helpers for the `tests/*.rs` integration suites that
//! pre-populate report directories with synthetic `RunReport`s.

#![allow(dead_code)] // each consuming test file uses only a subset

use serval_cli::report::{RunReport, RunSummary, SCHEMA_VERSION, TargetRef};
use serval_cli::runner::TestResult;
use std::path::{Path, PathBuf};

pub fn mk_test_result(title: &str, pass: bool, status: i16, example_index: i32) -> TestResult {
    TestResult {
        scenario_title: title.to_string(),
        example_index,
        pass,
        error_message: if pass {
            None
        } else {
            Some("synthetic failure".to_string())
        },
        response_status: status,
        response_data: None,
        request_duration_ms: 1,
        request_time: time::OffsetDateTime::from_unix_timestamp(0).unwrap(),
    }
}

pub fn mk_synthetic_report(unix_seconds: i64, results: Vec<TestResult>) -> RunReport {
    let t = time::OffsetDateTime::from_unix_timestamp(unix_seconds).unwrap();
    RunReport {
        schema_version: SCHEMA_VERSION,
        started_at: t,
        finished_at: t,
        source_file: "/synthetic.feature".to_string(),
        target: TargetRef {
            base_url: "http://localhost".to_string(),
            endpoint: "/api/test".to_string(),
            method: "GET".to_string(),
        },
        summary: RunSummary::from_results(&results),
        results,
    }
}

pub fn write_report(dir: &Path, id: &str, report: &RunReport) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{id}.json"));
    let json = serde_json::to_string_pretty(report).unwrap();
    std::fs::write(&path, json).unwrap();
    path
}
