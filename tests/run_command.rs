//! End-to-end tests for `serval run`. Spawns the real binary via
//! `assert_cmd` against an `httpmock` fake HTTP server, so the full
//! file → frontmatter → parser → runner → output pipeline is
//! exercised on every test.
//!
//! Every test passes `--no-report` so cargo test does not leave
//! `.serval/reports/*.json` artifacts in the project root.

use assert_cmd::Command;
use httpmock::{Method, MockServer};
use predicates::prelude::*;
use serde_json::json;

const FX_WITH_FM: &str = "tests/fixtures/run_with_frontmatter.feature";
const FX_NO_FM: &str = "tests/fixtures/run_no_frontmatter.feature";

#[test]
fn run_passes_when_status_assertion_matches() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200).json_body(json!({"status": "ok"}));
    });

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX_NO_FM,
            "--base-url",
            &server.base_url(),
            "--endpoint",
            "/health",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"))
        .stdout(predicate::str::contains("1 passed, 0 failed"));
}

#[test]
fn run_returns_exit_1_when_assertion_fails() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(500); // .feature expects 200
    });

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX_NO_FM,
            "--base-url",
            &server.base_url(),
            "--endpoint",
            "/health",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains("Expected status 200, got 500"));
}

#[test]
fn run_uses_frontmatter_when_present_without_flags() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/api/users");
        then.status(201).json_body(json!({"id": 1}));
    });

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX_WITH_FM,
            "--base-url",
            &server.base_url(),
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"));
}

#[test]
fn run_exits_3_when_target_unspecified() {
    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX_NO_FM,
            "--base-url",
            "http://localhost:9999",
            "--no-report",
            // intentionally no --endpoint / --method
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("missing `--endpoint`"));
}

#[test]
fn run_emits_json_array_with_flag() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200).json_body(json!({"status": "ok"}));
    });

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX_NO_FM,
            "--base-url",
            &server.base_url(),
            "--endpoint",
            "/health",
            "--method",
            "GET",
            "--json",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pass\": true"))
        .stdout(predicate::str::contains("\"response_status\": 200"));
}

#[test]
fn run_resolves_env_base_url_from_config_file() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200);
    });

    // Write a config file pointing `local` env at the mock server.
    let cfg = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "env",
            "set",
            "local",
            "--base-url",
            &server.base_url(),
            "--config-file",
            cfg.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    // `serval run --env local` resolves base_url through the config.
    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX_NO_FM,
            "--env",
            "local",
            "--endpoint",
            "/health",
            "--method",
            "GET",
            "--config-file",
            cfg.path().to_str().unwrap(),
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"));
}

#[test]
fn run_loads_user_patterns_from_file() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/x");
        then.status(500);
    });

    let tmp = tempfile::tempdir().unwrap();

    // User pattern adds a trigger for "the answer is N" that runs the
    // built-in status-scan action. Without this pattern, "the answer
    // is 200" doesn't match any built-in pattern, so the scenario
    // would pass vacuously against the 500 response. With it, the
    // action fires, scans for "200", asserts against the actual 500
    // response, and the scenario fails.
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        r#"
[[pattern]]
regex = '(?i)the answer is\s+\d{3}'
keyword_type = "Outcome"
[pattern.action]
type = "assert_status_from_text_scan"
"#,
    )
    .unwrap();

    let feature_path = tmp.path().join("smoke.feature");
    std::fs::write(
        &feature_path,
        "Feature: User pattern smoke\n  Scenario: custom assertion\n    When I GET /x\n    Then the answer is 200\n",
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            "--endpoint",
            "/x",
            "--method",
            "GET",
            "--no-report",
            "--patterns-file",
            patterns_path.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("Expected status 200, got 500"));
}

#[test]
fn run_exits_3_when_no_base_url_nor_env_resolves() {
    // Config file with no entries — neither --base-url nor --env
    // can resolve a target.
    let cfg = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX_NO_FM,
            "--config-file",
            cfg.path().to_str().unwrap(),
            "--no-report",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("could not resolve target server"));
}
