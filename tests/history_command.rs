//! End-to-end tests for `serval history`. Populates a tempdir with
//! synthetic `RunReport`s and invokes the real binary via
//! `assert_cmd`.

mod common;
use common::{mk_synthetic_report, mk_test_result, write_report};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

#[test]
fn history_lists_reports_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "older",
        &mk_synthetic_report(1_000_000_000, vec![mk_test_result("a", true, 200, 0)]),
    );
    write_report(
        tmp.path(),
        "newer",
        &mk_synthetic_report(2_000_000_000, vec![mk_test_result("b", true, 200, 0)]),
    );

    Command::cargo_bin("serval")
        .unwrap()
        .args(["history", "--report-dir", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::function(|s: &str| {
            let newer = s.find("newer");
            let older = s.find("older");
            matches!((newer, older), (Some(n), Some(o)) if n < o)
        }));
}

#[test]
fn history_respects_limit() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..3 {
        let id = format!("r{i}");
        write_report(
            tmp.path(),
            &id,
            &mk_synthetic_report(1_000_000_000 + i64::from(i), vec![]),
        );
    }

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "history",
            "--report-dir",
            tmp.path().to_str().unwrap(),
            "--limit",
            "2",
        ])
        .assert()
        .success()
        .stdout(predicate::function(|s: &str| {
            s.contains("r2") && s.contains("r1") && !s.contains("r0")
        }));
}

#[test]
fn history_empty_dir_says_no_reports_found() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("serval")
        .unwrap()
        .args(["history", "--report-dir", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("no reports found"));
}

#[test]
fn history_json_output_is_array_with_iso_timestamps() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "r1",
        &mk_synthetic_report(1_000_000_000, vec![mk_test_result("x", true, 200, 0)]),
    );

    let out = Command::cargo_bin("serval")
        .unwrap()
        .args([
            "history",
            "--report-dir",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "history --json must succeed");

    let json: Value = serde_json::from_slice(&out.stdout).expect("stdout must be JSON");
    let arr = json.as_array().expect("JSON should be an array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], "r1");
    assert!(
        arr[0]["started_at"].as_str().is_some(),
        "started_at must serialize as an RFC 3339 string, got {:?}",
        arr[0]["started_at"]
    );
}
