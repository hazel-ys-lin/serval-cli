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
[[pattern.actions]]
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
fn run_fires_multi_step_http_requests_from_patterns() {
    // Two `Action::HttpRequest` patterns drive two distinct HTTP
    // calls in one scenario; validation runs against the *last*
    // response (DELETE → 204). This exercises:
    //   - endpoint-template substitution via named capture groups
    //   - response accumulation across steps
    //   - that `--endpoint` / `--method` flags become dummies once
    //     `HttpRequest` patterns are firing (the implicit fallback
    //     never runs because `responses` is non-empty).
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/users/alice");
        then.status(200).json_body(json!({"name": "alice"}));
    });
    server.mock(|when, then| {
        when.method(Method::DELETE).path("/users/alice");
        then.status(204);
    });

    let tmp = tempfile::tempdir().unwrap();

    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        r#"
[[pattern]]
regex = '(?i)i fetch user "(?P<name>[^"]+)"'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "GET"
endpoint_template = "/users/{{name}}"

[[pattern]]
regex = '(?i)i delete user "(?P<name>[^"]+)"'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "DELETE"
endpoint_template = "/users/{{name}}"
"#,
    )
    .unwrap();

    let feature_path = tmp.path().join("multi.feature");
    std::fs::write(
        &feature_path,
        "Feature: Multi-step\n  Scenario: fetch then delete\n    When I fetch user \"alice\"\n    And I delete user \"alice\"\n    Then status should be 204\n",
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            // Dummy frontmatter target — never fires because
            // HttpRequest patterns populate `responses` first.
            "--endpoint",
            "/unused",
            "--method",
            "GET",
            "--no-report",
            "--patterns-file",
            patterns_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"))
        .stdout(predicate::str::contains("status 204"));
}

#[test]
fn run_then_doc_string_deep_matches_response_body() {
    // Phase 2.4: a `Then` step with a doc string parses the doc
    // string as JSON into `expected_body` and runs a deep partial
    // match against the response body. The mock returns more fields
    // than the assertion lists; the test still passes because
    // `json_contains` is partial-match.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/users/1");
        then.status(200)
            .json_body(json!({"id": 1, "name": "alice", "email": "a@b.c"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let feature_path = tmp.path().join("body_match.feature");
    std::fs::write(
        &feature_path,
        "Feature: Body match\n  Scenario: alice profile\n    When I GET /users/1\n    Then the response is:\n      \"\"\"\n      {\"id\": 1, \"name\": \"alice\"}\n      \"\"\"\n",
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
            "/users/1",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"));
}

#[test]
fn run_then_doc_string_mismatch_fails() {
    // Response body does not match the doc-string spec → exit 1
    // with `Response body does not match expected` in the [FAIL]
    // line. Proves the new pattern actually drives validation,
    // not just sets state.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/users/1");
        then.status(200).json_body(json!({"id": 2, "name": "bob"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let feature_path = tmp.path().join("body_mismatch.feature");
    std::fs::write(
        &feature_path,
        "Feature: Body mismatch\n  Scenario: expects alice\n    When I GET /users/1\n    Then the response is:\n      \"\"\"\n      {\"id\": 1, \"name\": \"alice\"}\n      \"\"\"\n",
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
            "/users/1",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains(
            "Response body does not match expected",
        ));
}

#[test]
fn run_operation_fails_pattern_asserts_status_range_and_body_substring() {
    // Phase 2.6: a user pattern for `Then the operation fails with: <msg>`
    // fires both AssertExpectedStatusInRange (400..499) and
    // AssertBodyContainsFromMatchGroup. The mock returns 400 with the
    // expected message in body, so the scenario PASSes.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/x");
        then.status(400)
            .json_body(json!({"error": "account does not exist"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        r#"
[[pattern]]
regex = '(?i)the operation fails with:\s*(?P<msg>.+?)\s*$'
keyword_type = "Outcome"
[[pattern.actions]]
type = "assert_expected_status_in_range"
min = 400
max = 499
[[pattern.actions]]
type = "assert_body_contains_from_match_group"
group = "msg"
"#,
    )
    .unwrap();

    let feature_path = tmp.path().join("op_fails.feature");
    std::fs::write(
        &feature_path,
        "Feature: Op fails\n  Scenario: account missing\n    When I POST /x\n    Then the operation fails with: account does not exist\n",
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
            "POST",
            "--no-report",
            "--patterns-file",
            patterns_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"));
}

#[test]
fn run_operation_fails_status_outside_range_fails_validation() {
    // Mock returns 200 instead of 4xx; AssertExpectedStatusInRange
    // 400..499 should fail with a range-describing message.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/x");
        then.status(200).json_body(json!({"unexpected": "ok"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        r#"
[[pattern]]
regex = '(?i)the operation fails with:\s*(?P<msg>.+?)\s*$'
keyword_type = "Outcome"
[[pattern.actions]]
type = "assert_expected_status_in_range"
min = 400
max = 499
[[pattern.actions]]
type = "assert_body_contains_from_match_group"
group = "msg"
"#,
    )
    .unwrap();

    let feature_path = tmp.path().join("op_unexpected.feature");
    std::fs::write(
        &feature_path,
        "Feature: Op unexpected\n  Scenario: should have failed\n    When I POST /x\n    Then the operation fails with: bad thing\n",
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
            "POST",
            "--no-report",
            "--patterns-file",
            patterns_path.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains(
            "Expected status 400..=499, got 200",
        ));
}

#[test]
fn run_vacuous_pass_fails_by_default() {
    // Phase 2.6 P1: a scenario that runs without any assertion set
    // is marked FAIL with a hint pointing at --allow-no-assertions.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200);
    });

    let tmp = tempfile::tempdir().unwrap();
    // Spec has neither a status pattern nor a doc string — nothing
    // fires an assertion.
    let feature_path = tmp.path().join("vacuous.feature");
    std::fs::write(
        &feature_path,
        "Feature: Vacuous\n  Scenario: no assertion\n    When I GET /health\n",
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
            "/health",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains(
            "scenario ran without setting any assertion",
        ))
        .stdout(predicate::str::contains("--allow-no-assertions"));
}

#[test]
fn run_vacuous_pass_allowed_with_flag() {
    // Same as above with `--allow-no-assertions` — scenario PASSes.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200);
    });

    let tmp = tempfile::tempdir().unwrap();
    let feature_path = tmp.path().join("vacuous.feature");
    std::fs::write(
        &feature_path,
        "Feature: Vacuous\n  Scenario: no assertion\n    When I GET /health\n",
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
            "/health",
            "--method",
            "GET",
            "--no-report",
            "--allow-no-assertions",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"));
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
