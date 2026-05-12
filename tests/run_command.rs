//! End-to-end tests for `serval run`. Spawns the real binary via
//! `assert_cmd` against an `httpmock` fake HTTP server, so the full
//! file → frontmatter → parser → runner → output pipeline is
//! exercised on every test.

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
        .args(["run", FX_WITH_FM, "--base-url", &server.base_url()])
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
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pass\": true"))
        .stdout(predicate::str::contains("\"response_status\": 200"));
}
