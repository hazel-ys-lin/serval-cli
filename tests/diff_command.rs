//! End-to-end tests for `serval diff`. Populates a tempdir with two
//! synthetic `RunReport`s and invokes the real binary via
//! `assert_cmd`.

mod common;
use common::{mk_synthetic_report, mk_test_result, write_report};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

#[test]
fn diff_detects_pass_to_fail_flip() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "before",
        &mk_synthetic_report(1_000_000_000, vec![mk_test_result("login", true, 200, 0)]),
    );
    write_report(
        tmp.path(),
        "after",
        &mk_synthetic_report(2_000_000_000, vec![mk_test_result("login", false, 500, 0)]),
    );

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "diff",
            "before",
            "after",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PASS -> FAIL"))
        .stdout(predicate::str::contains("login"))
        .stdout(predicate::str::contains("200 -> 500"));
}

#[test]
fn diff_detects_added_scenario() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "before",
        &mk_synthetic_report(1_000_000_000, vec![mk_test_result("a", true, 200, 0)]),
    );
    write_report(
        tmp.path(),
        "after",
        &mk_synthetic_report(
            2_000_000_000,
            vec![
                mk_test_result("a", true, 200, 0),
                mk_test_result("brand_new", true, 200, 0),
            ],
        ),
    );

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "diff",
            "before",
            "after",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[+]"))
        .stdout(predicate::str::contains("brand_new"));
}

#[test]
fn diff_detects_removed_scenario() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "before",
        &mk_synthetic_report(
            1_000_000_000,
            vec![
                mk_test_result("a", true, 200, 0),
                mk_test_result("gone", true, 200, 0),
            ],
        ),
    );
    write_report(
        tmp.path(),
        "after",
        &mk_synthetic_report(2_000_000_000, vec![mk_test_result("a", true, 200, 0)]),
    );

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "diff",
            "before",
            "after",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[-]"))
        .stdout(predicate::str::contains("gone"));
}

#[test]
fn diff_resolves_latest_and_previous_keywords() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "first",
        &mk_synthetic_report(1_000_000_000, vec![mk_test_result("login", true, 200, 0)]),
    );
    write_report(
        tmp.path(),
        "second",
        &mk_synthetic_report(2_000_000_000, vec![mk_test_result("login", false, 500, 0)]),
    );

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "diff",
            "previous",
            "latest",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("first"))
        .stdout(predicate::str::contains("second"));
}

#[test]
fn diff_resolves_unique_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "aaa-older",
        &mk_synthetic_report(1_000_000_000, vec![mk_test_result("x", true, 200, 0)]),
    );
    write_report(
        tmp.path(),
        "bbb-newer",
        &mk_synthetic_report(2_000_000_000, vec![mk_test_result("x", true, 200, 0)]),
    );

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "diff",
            "aaa",
            "bbb",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
fn diff_errors_with_exit_3_on_ambiguous_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "2026-05-12T03-30",
        &mk_synthetic_report(1_000_000_000, vec![]),
    );
    write_report(
        tmp.path(),
        "2026-05-12T11-22",
        &mk_synthetic_report(2_000_000_000, vec![]),
    );

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "diff",
            "2026-05-12",
            "latest",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("ambiguous"));
}

#[test]
fn diff_json_output_shape() {
    let tmp = tempfile::tempdir().unwrap();
    write_report(
        tmp.path(),
        "before",
        &mk_synthetic_report(1_000_000_000, vec![mk_test_result("x", true, 200, 0)]),
    );
    write_report(
        tmp.path(),
        "after",
        &mk_synthetic_report(2_000_000_000, vec![mk_test_result("x", false, 500, 0)]),
    );

    let out = Command::cargo_bin("serval")
        .unwrap()
        .args([
            "diff",
            "before",
            "after",
            "--report-dir",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "diff --json must succeed");

    let json: Value = serde_json::from_slice(&out.stdout).expect("stdout must be JSON");
    assert_eq!(json["before_id"], "before");
    assert_eq!(json["after_id"], "after");
    assert!(json["scenarios"].is_array());
    assert_eq!(json["scenarios"][0]["change"], "flipped");
    assert_eq!(json["scenarios"][0]["title"], "x");
}
