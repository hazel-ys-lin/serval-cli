//! End-to-end tests for `serval spec validate`.

mod common;
use common::write_feature;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

const VALID: &str = "\
Feature: Valid
  Scenario: x
    Given y
";

const VALID_WITH_FM: &str = "\
---
api:
  path: /api/users
  method: GET
---
Feature: Valid with FM
  Scenario: list
    When I GET /api/users
    Then status should be 200
";

// Unclosed doc string — gherkin crate rejects this with a parse
// error, so the file definitely fails `spec validate`.
const BROKEN_GHERKIN: &str = "\
Feature: Broken
  Scenario: x
    Given y
      \"\"\"
      unclosed docstring
";

const BROKEN_FRONTMATTER: &str = "\
---
api: this is not a valid yaml block
  path: /x
---
Feature: F
  Scenario: x
    Given y
";

fn serval() -> Command {
    Command::cargo_bin("serval").unwrap()
}

#[test]
fn validate_directory_with_all_valid_files_exits_0() {
    let tmp = tempfile::tempdir().unwrap();
    write_feature(tmp.path(), "ok1.feature", VALID);
    write_feature(tmp.path(), "ok2.feature", VALID_WITH_FM);

    serval()
        .args(["spec", "validate", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "2 file(s) scanned: 2 valid, 0 invalid",
        ));
}

#[test]
fn validate_directory_with_any_invalid_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    write_feature(tmp.path(), "ok.feature", VALID);
    write_feature(tmp.path(), "broken.feature", BROKEN_GHERKIN);

    serval()
        .args(["spec", "validate", tmp.path().to_str().unwrap()])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("ERR"))
        .stdout(predicate::str::contains("broken.feature"))
        .stdout(predicate::str::contains("1 valid, 1 invalid"));
}

#[test]
fn validate_single_file_valid_exits_0() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_feature(tmp.path(), "ok.feature", VALID);

    serval()
        .args(["spec", "validate", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("OK"))
        .stdout(predicate::str::contains("1 file(s) scanned: 1 valid"));
}

#[test]
fn validate_single_file_broken_exits_3_with_message() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_feature(tmp.path(), "broken.feature", BROKEN_GHERKIN);

    serval()
        .args(["spec", "validate", path.to_str().unwrap()])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("ERR"))
        .stdout(predicate::str::contains("Gherkin parse error"));
}

#[test]
fn validate_broken_frontmatter_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_feature(tmp.path(), "fm_broken.feature", BROKEN_FRONTMATTER);

    serval()
        .args(["spec", "validate", path.to_str().unwrap()])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("ERR"));
}

#[test]
fn validate_missing_path_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist");

    serval()
        .args(["spec", "validate", missing.to_str().unwrap()])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("path does not exist"));
}

#[test]
fn validate_empty_directory_reports_zero_files() {
    let tmp = tempfile::tempdir().unwrap();

    serval()
        .args(["spec", "validate", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("no `.feature` files found"));
}

#[test]
fn validate_json_output_shape() {
    let tmp = tempfile::tempdir().unwrap();
    write_feature(tmp.path(), "ok.feature", VALID_WITH_FM);
    write_feature(tmp.path(), "broken.feature", BROKEN_GHERKIN);

    let out = serval()
        .args(["spec", "validate", tmp.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    let arr = json.as_array().expect("JSON should be array");
    assert_eq!(arr.len(), 2);
    // Sorted by path alphabetically; broken.feature comes first.
    assert_eq!(arr[0]["status"], "error");
    assert!(arr[0]["error"].as_str().is_some());
    assert_eq!(arr[1]["status"], "ok");
    assert_eq!(arr[1]["features"], 1);
    assert_eq!(arr[1]["scenarios"], 1);
}
