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
fn run_vacuous_empty_doc_string_then_fails_by_default() {
    // Phase 3.2: a `Then` step whose doc string is an empty object
    // `{}` does NOT count as an assertion. Codegen Gherkin uses this
    // shape as documentation of "an event of this shape exists"
    // without asserting any field. The deep-partial match would
    // otherwise pass any body trivially; strict mode must catch
    // the missing assertion instead.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/login");
        then.status(200)
            .json_body(json!({"token": "tk", "user_id": "u-1"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let feature_path = tmp.path().join("vacuous-body.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: Vacuous\n",
            "  Scenario: empty Then doc string\n",
            "    When I POST /login\n",
            "    Then the LoggedIn event is emitted with:\n",
            "      \"\"\"\n",
            "      {}\n",
            "      \"\"\"\n",
        ),
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
            "/login",
            "--method",
            "POST",
            "--no-report",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains(
            "scenario ran without setting any assertion",
        ));
}

#[test]
fn run_empty_doc_string_paired_with_status_assertion_still_passes() {
    // The vacuous-body silencer must not break the legitimate case
    // where the doc string is empty but another assertion (here the
    // built-in status scan on `Then status should be 200`) carries
    // the real check.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/login");
        then.status(200)
            .json_body(json!({"token": "tk", "user_id": "u-1"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let feature_path = tmp.path().join("vacuous-body-with-status.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: Vacuous body with status\n",
            "  Scenario: empty Then doc string but status fires\n",
            "    When I POST /login\n",
            "    Then status should be 200\n",
            "    And the LoggedIn event is emitted with:\n",
            "      \"\"\"\n",
            "      {}\n",
            "      \"\"\"\n",
        ),
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
            "/login",
            "--method",
            "POST",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"));
}

#[test]
fn run_assert_body_matches_at_scopes_partial_match_to_pointer() {
    // Phase 3.4: the wire-shape gap. v2-style backend returns
    // `{users: [...]}` but codegen Gherkin's `Then the view
    // returns: [...]` asserts against a bare array. Without
    // pointer scoping, the deep-match fails (object vs array
    // at root). The user-supplied AssertBodyMatchesAt pattern
    // scopes the comparison to /users.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/users/list");
        then.status(200).json_body(json!({
            "users": [
                {"name": "Alice", "extra": "ignored-by-partial-match"},
                {"name": "Bob"},
            ]
        }));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        concat!(
            "[[pattern]]\n",
            "regex = '(?i)the AccountList view is queried'\n",
            "keyword_type = \"Action\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"GET\"\n",
            "endpoint_template = \"/users/list\"\n",
            "\n",
            "[[pattern]]\n",
            "regex = '(?i)the view returns'\n",
            "keyword_type = \"Outcome\"\n",
            "[[pattern.actions]]\n",
            "type = \"assert_body_matches_at\"\n",
            "pointer = \"/users\"\n",
        ),
    )
    .unwrap();

    let feature_path = tmp.path().join("view.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: View partial-match\n",
            "  Scenario: AccountList view returns wrapped\n",
            "    When the AccountList view is queried\n",
            "    Then the view returns:\n",
            "      \"\"\"\n",
            "      [{\"name\": \"Alice\"}]\n",
            "      \"\"\"\n",
        ),
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            "--patterns-file",
            patterns_path.to_str().unwrap(),
            "--endpoint",
            "/placeholder",
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
fn run_assert_body_matches_at_fails_when_pointer_missing() {
    // Negative case: the pointer doesn't resolve in the response.
    // Validator should report a clear pointer-missing error rather
    // than vacuously passing or panicking.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/users/list");
        then.status(200).json_body(json!({"unexpected": "shape"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        concat!(
            "[[pattern]]\n",
            "regex = '(?i)the AccountList view is queried'\n",
            "keyword_type = \"Action\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"GET\"\n",
            "endpoint_template = \"/users/list\"\n",
            "\n",
            "[[pattern]]\n",
            "regex = '(?i)the view returns'\n",
            "keyword_type = \"Outcome\"\n",
            "[[pattern.actions]]\n",
            "type = \"assert_body_matches_at\"\n",
            "pointer = \"/users\"\n",
        ),
    )
    .unwrap();

    let feature_path = tmp.path().join("view-missing.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: View pointer missing\n",
            "  Scenario: pointer does not resolve\n",
            "    When the AccountList view is queried\n",
            "    Then the view returns:\n",
            "      \"\"\"\n",
            "      [{\"name\": \"Alice\"}]\n",
            "      \"\"\"\n",
        ),
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            "--patterns-file",
            patterns_path.to_str().unwrap(),
            "--endpoint",
            "/placeholder",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains(
            "Pointer /users resolved to nothing",
        ));
}

#[test]
fn run_accepted_status_treats_listed_codes_as_success() {
    // Phase 3.5: a seed POST pattern with accepted_status = [201,
    // 409] tolerates a "user already exists" response across
    // re-runs. Scenario continues normally so the downstream When
    // / Then assertion can land.
    let server = MockServer::start();
    // Seed POST returns 409 — treated as acceptable by the
    // pattern's accepted_status list.
    server.mock(|when, then| {
        when.method(Method::POST).path("/users/create");
        then.status(409)
            .json_body(json!({"detail": "already exists"}));
    });
    // Subsequent login the scenario also fires — its 200 response
    // is what the Then assertion validates against.
    server.mock(|when, then| {
        when.method(Method::POST).path("/auth/login");
        then.status(200).json_body(json!({"token": "tk"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        concat!(
            "[[pattern]]\n",
            "regex = '(?i)the AccountCreated event has occurred'\n",
            "keyword_type = \"Context\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"POST\"\n",
            "endpoint_template = \"/users/create\"\n",
            "accepted_status = [201, 409]\n",
            "\n",
            "[[pattern]]\n",
            "regex = '(?i)Anonymous sends Login'\n",
            "keyword_type = \"Action\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"POST\"\n",
            "endpoint_template = \"/auth/login\"\n",
        ),
    )
    .unwrap();

    let feature_path = tmp.path().join("seed-409.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: Seed idempotency\n",
            "  Scenario: seed POST returns 409 but scenario continues\n",
            "    Given the AccountCreated event has occurred on stream \"acc-001\":\n",
            "      \"\"\"\n",
            "      {\"name\": \"Alice\"}\n",
            "      \"\"\"\n",
            "    When Anonymous sends Login on stream \"acc-001\":\n",
            "      \"\"\"\n",
            "      {\"username\": \"alice\"}\n",
            "      \"\"\"\n",
            "    Then status should be 200\n",
        ),
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            "--patterns-file",
            patterns_path.to_str().unwrap(),
            "--endpoint",
            "/placeholder",
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
fn run_accepted_status_unlisted_code_fails_the_scenario() {
    // Negative case: pattern declares accepted_status = [201] but
    // the backend returns 500. Step is aborted with a clear error
    // (caught by the runner as a step_failure → scenario FAIL).
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/users/create");
        then.status(500).json_body(json!({"detail": "boom"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        concat!(
            "[[pattern]]\n",
            "regex = '(?i)the AccountCreated event has occurred'\n",
            "keyword_type = \"Context\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"POST\"\n",
            "endpoint_template = \"/users/create\"\n",
            "accepted_status = [201]\n",
        ),
    )
    .unwrap();

    let feature_path = tmp.path().join("seed-500.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: Seed hard failure\n",
            "  Scenario: seed POST returns 500\n",
            "    Given the AccountCreated event has occurred on stream \"acc-001\":\n",
            "      \"\"\"\n",
            "      {\"name\": \"Alice\"}\n",
            "      \"\"\"\n",
            "    Then status should be 200\n",
        ),
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            "--patterns-file",
            patterns_path.to_str().unwrap(),
            "--endpoint",
            "/placeholder",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("[FAIL]"))
        .stdout(predicate::str::contains("returned status 500"))
        .stdout(predicate::str::contains("accepted_status"));
}

#[test]
fn run_stream_id_symbol_table_chains_seed_capture_into_delete() {
    // Phase 3.6: closes the stream-id ↔ UUID gap.
    //
    // Seed pattern captures the backend-assigned UUID under a
    // variable whose name embeds the Gherkin stream id
    // (`user_for_{{stream}}`). The delete pattern then references
    // that same variable through a nested template
    // (`{{$user_for_{{stream}}}}`), so the request lands at
    // /users/delete/<UUID> rather than /users/delete/acc-001.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/users/create");
        then.status(201)
            .json_body(json!({"id": "019e1f4d-3c69", "account": "coach"}));
    });
    server.mock(|when, then| {
        when.method(Method::DELETE)
            .path("/users/delete/019e1f4d-3c69");
        then.status(204);
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        concat!(
            "[[pattern]]\n",
            "regex = '(?i)the AccountCreated event has occurred on stream \"(?P<stream>[^\"]+)\"'\n",
            "keyword_type = \"Context\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"POST\"\n",
            "endpoint_template = \"/users/create\"\n",
            "capture_response = { \"user_for_{{stream}}\" = \"/id\" }\n",
            "\n",
            "[[pattern]]\n",
            "regex = '(?i)Coach sends DeleteAccount on stream \"(?P<stream>[^\"]+)\"'\n",
            "keyword_type = \"Action\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"DELETE\"\n",
            "endpoint_template = \"/users/delete/{{$user_for_{{stream}}}}\"\n",
            "\n",
            "[[pattern]]\n",
            "regex = '(?i)the AccountDeleted event is emitted'\n",
            "keyword_type = \"Outcome\"\n",
            "[[pattern.actions]]\n",
            "type = \"assert_status_from_text_scan\"\n",
        ),
    )
    .unwrap();

    let feature_path = tmp.path().join("stream-id.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: Stream id chain\n",
            "  Scenario: Delete by captured UUID\n",
            "    Given the AccountCreated event has occurred on stream \"acc-001\":\n",
            "      \"\"\"\n",
            "      {\"name\": \"coach\"}\n",
            "      \"\"\"\n",
            "    When Coach sends DeleteAccount on stream \"acc-001\":\n",
            "      \"\"\"\n",
            "      {}\n",
            "      \"\"\"\n",
            "    Then status should be 204\n",
        ),
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            "--patterns-file",
            patterns_path.to_str().unwrap(),
            "--endpoint",
            "/placeholder",
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
fn run_doc_captures_resolves_stream_id_in_body_overrides() {
    // Phase 3.7 end-to-end: a CreatePlayer-like pattern needs to
    // translate Gherkin's `"teamId": "team-001"` (a stream id) into
    // the backend's UUID before the body lands on the API. Chain:
    //   1. A prior seed pattern captures `team_for_team-001` =
    //      <uuid> via response.
    //   2. The current step's pattern extracts the doc-string's
    //      `/teamId` into `team_stream` via doc_captures.
    //   3. The body's overrides set `team_id` = lookup chain
    //      `{{$team_for_{{$team_stream}}}}`, which multipass
    //      variable substitution resolves to the captured UUID.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/teams/create");
        then.status(201)
            .json_body(json!({"id": "019e1f49-team", "teamName": "T"}));
    });
    server.mock(|when, then| {
        when.method(Method::POST)
            .path("/teams/members/create")
            .json_body_partial(r#"{"team_id": "019e1f49-team"}"#);
        then.status(201)
            .json_body(json!({"id": "019e1f49-mem", "name": "Alice"}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        concat!(
            "[[pattern]]\n",
            "regex = '(?i)the TeamCreated event has occurred on stream \"(?P<stream>[^\"]+)\"'\n",
            "keyword_type = \"Context\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"POST\"\n",
            "endpoint_template = \"/teams/create\"\n",
            "capture_response = { \"team_for_{{stream}}\" = \"/id\" }\n",
            "accepted_status = [201, 409]\n",
            "\n",
            "[[pattern]]\n",
            "regex = '(?i)the PlayerCreated event has occurred on stream \"(?P<stream>[^\"]+)\"'\n",
            "keyword_type = \"Context\"\n",
            "[[pattern.actions]]\n",
            "type = \"http_request\"\n",
            "method = \"POST\"\n",
            "endpoint_template = \"/teams/members/create\"\n",
            "doc_captures = { team_stream = \"/teamId\" }\n",
            "body_from = { kind = \"doc_string_template\", rename = { playerName = \"name\" }, defaults = { jersey_number = \"1\", position = \"PITCHER\" }, overrides = { team_id = \"{{$team_for_{{$team_stream}}}}\" } }\n",
            "accepted_status = [201, 409]\n",
            "\n",
            "[[pattern]]\n",
            "regex = '(?i)status should pass'\n",
            "keyword_type = \"Outcome\"\n",
            "[[pattern.actions]]\n",
            "type = \"assert_expected_status_in_range\"\n",
            "min = 200\n",
            "max = 299\n",
        ),
    )
    .unwrap();

    let feature_path = tmp.path().join("doc-captures.feature");
    std::fs::write(
        &feature_path,
        concat!(
            "Feature: doc_captures chain\n",
            "  Scenario: PlayerCreated rewrites teamId to captured team UUID\n",
            "    Given the TeamCreated event has occurred on stream \"team-001\":\n",
            "      \"\"\"\n",
            "      {\"teamName\": \"T\"}\n",
            "      \"\"\"\n",
            "    And the PlayerCreated event has occurred on stream \"player-001\":\n",
            "      \"\"\"\n",
            "      {\"playerName\": \"Alice\", \"teamId\": \"team-001\", \"height\": 165}\n",
            "      \"\"\"\n",
            "    Then status should pass\n",
        ),
    )
    .unwrap();

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            feature_path.to_str().unwrap(),
            "--base-url",
            &server.base_url(),
            "--patterns-file",
            patterns_path.to_str().unwrap(),
            "--endpoint",
            "/placeholder",
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
fn run_captures_response_field_into_scenario_variable() {
    // Phase 3.0: a login pattern captures `/access_token` from the
    // response body; a subsequent pattern reads it via `{{$token}}`
    // in an Authorization header. The mock for the protected endpoint
    // matches on the exact header value, so the scenario only PASSes
    // if the variable round-tripped correctly.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::POST).path("/auth/login");
        then.status(200)
            .json_body(json!({"access_token": "tk-abc-123"}));
    });
    server.mock(|when, then| {
        when.method(Method::GET)
            .path("/protected")
            .header("Authorization", "Bearer tk-abc-123");
        then.status(200).json_body(json!({"ok": true}));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        r#"
[[pattern]]
regex = '(?i)i log in'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/auth/login"
body_from = { kind = "doc_string" }
capture_response = { token = "/access_token" }

[[pattern]]
regex = '(?i)i fetch the protected resource'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "GET"
endpoint_template = "/protected"
headers = { Authorization = "Bearer {{$token}}" }
"#,
    )
    .unwrap();

    let feature_path = tmp.path().join("chain.feature");
    std::fs::write(
        &feature_path,
        "Feature: Chain\n  Scenario: login then fetch\n    When I log in\n      \"\"\"\n      {\"username\": \"u\", \"password\": \"p\"}\n      \"\"\"\n    And I fetch the protected resource\n    Then status should be 200\n",
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
            "/unused",
            "--method",
            "GET",
            "--no-report",
            "--patterns-file",
            patterns_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"));
}

#[test]
fn run_global_header_flag_flows_into_http_request_pattern() {
    // Phase 3.0: --header is parsed into TestConfig.custom_headers and
    // attached to every HTTP firing, including pattern-driven
    // `Action::HttpRequest` calls (not just the frontmatter fallback).
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET)
            .path("/things")
            .header("X-Tenant", "acme");
        then.status(200).json_body(json!([]));
    });

    let tmp = tempfile::tempdir().unwrap();
    let patterns_path = tmp.path().join("patterns.toml");
    std::fs::write(
        &patterns_path,
        r#"
[[pattern]]
regex = '(?i)i fetch things'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "GET"
endpoint_template = "/things"
"#,
    )
    .unwrap();

    let feature_path = tmp.path().join("hdr.feature");
    std::fs::write(
        &feature_path,
        "Feature: Hdr\n  Scenario: with tenant header\n    When I fetch things\n    Then status should be 200\n",
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
            "/unused",
            "--method",
            "GET",
            "--no-report",
            "--patterns-file",
            patterns_path.to_str().unwrap(),
            "--header",
            "X-Tenant: acme",
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
