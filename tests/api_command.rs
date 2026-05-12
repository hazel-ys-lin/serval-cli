//! End-to-end tests for `serval api {list,show,find}`. Populates a
//! tempdir with synthetic `.feature` files and invokes the real
//! binary via `assert_cmd`.

mod common;
use common::write_feature;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

const USERS_CREATE: &str = "\
---
api:
  path: /api/users
  method: POST
  collection: users
implements:
  - src/handlers/users.rs::create
---
Feature: Users — create
  @happy-path
  Scenario: signup OK
    When I POST /api/users
    Then status should be 201
";

const USERS_LIST: &str = "\
---
api:
  path: /api/users
  method: GET
  collection: users
---
Feature: Users — list
  Scenario: list users
    When I GET /api/users
    Then status should be 200
";

const ORDERS_UPDATE: &str = "\
---
api:
  path: /api/orders/:id
  method: PUT
  collection: orders
---
Feature: Orders — update
  @editing
  Scenario: rename order
    When I PUT /api/orders/1
    Then status should be 200
";

const NO_FRONTMATTER: &str = "\
Feature: Plain
  Scenario: no api block
    Given x
";

fn populate(dir: &std::path::Path) {
    write_feature(dir, "users/create.feature", USERS_CREATE);
    write_feature(dir, "users/list.feature", USERS_LIST);
    write_feature(dir, "orders/update.feature", ORDERS_UPDATE);
    write_feature(dir, "misc/plain.feature", NO_FRONTMATTER);
}

#[test]
fn list_shows_only_frontmatter_specs_with_scanned_count() {
    let tmp = tempfile::tempdir().unwrap();
    populate(tmp.path());

    Command::cargo_bin("serval")
        .unwrap()
        .args(["api", "list", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        // headers + 3 API rows
        .stdout(predicate::str::contains("METHOD"))
        .stdout(predicate::str::contains("/api/users"))
        .stdout(predicate::str::contains("/api/orders/:id"))
        // summary line mentions 4 scanned, 1 omitted
        .stdout(predicate::str::contains("4 spec file(s) scanned"))
        .stdout(predicate::str::contains("1 without API frontmatter"));
}

#[test]
fn list_empty_dir_message() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("serval")
        .unwrap()
        .args(["api", "list", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("no `.feature` files found"));
}

#[test]
fn list_json_output_shape() {
    let tmp = tempfile::tempdir().unwrap();
    write_feature(tmp.path(), "users/create.feature", USERS_CREATE);

    let out = Command::cargo_bin("serval")
        .unwrap()
        .args([
            "api",
            "list",
            "--dir",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    let arr = json.as_array().expect("api list --json is an array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["method"], "POST");
    assert_eq!(arr[0]["path"], "/api/users");
    assert_eq!(arr[0]["collection"], "users");
    assert_eq!(arr[0]["scenario_count"], 1);
}

#[test]
fn show_exact_unique_match_renders_detail() {
    let tmp = tempfile::tempdir().unwrap();
    populate(tmp.path());

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "api",
            "show",
            "orders/update",
            "--dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/api/orders/:id"))
        .stdout(predicate::str::contains("collection: orders"))
        .stdout(predicate::str::contains("Orders — update"))
        .stdout(predicate::str::contains("rename order"));
}

#[test]
fn show_ambiguous_match_exits_3_with_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    populate(tmp.path());

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "api",
            "show",
            "users",
            "--dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("specs match"))
        .stderr(predicate::str::contains("candidates:"))
        .stderr(predicate::str::contains("users/create.feature"))
        .stderr(predicate::str::contains("users/list.feature"));
}

#[test]
fn show_no_match_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    populate(tmp.path());

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "api",
            "show",
            "zzz-not-here",
            "--dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no spec matches"));
}

#[test]
fn find_filters_by_method_case_insensitive() {
    let tmp = tempfile::tempdir().unwrap();
    populate(tmp.path());

    Command::cargo_bin("serval")
        .unwrap()
        .args(["api", "find", "post", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("/api/users"))
        .stdout(predicate::str::contains("POST"))
        // does not surface GET endpoint
        .stdout(predicate::str::contains("PUT").not());
}

#[test]
fn find_filters_by_tag() {
    let tmp = tempfile::tempdir().unwrap();
    populate(tmp.path());

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "api",
            "find",
            "editing",
            "--dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        // only orders has @editing tag
        .stdout(predicate::str::contains("/api/orders/:id"))
        .stdout(predicate::str::contains("/api/users").not());
}

#[test]
fn show_json_includes_features_and_scenarios() {
    let tmp = tempfile::tempdir().unwrap();
    write_feature(tmp.path(), "users/create.feature", USERS_CREATE);

    let out = Command::cargo_bin("serval")
        .unwrap()
        .args([
            "api",
            "show",
            "create",
            "--dir",
            tmp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["api"]["path"], "/api/users");
    assert_eq!(json["api"]["method"], "POST");
    assert!(json["features"].is_array());
    assert_eq!(json["features"][0]["name"], "Users — create");
    assert_eq!(json["features"][0]["scenarios"][0]["title"], "signup OK");
    assert_eq!(json["features"][0]["scenarios"][0]["tags"][0], "happy-path");
}
