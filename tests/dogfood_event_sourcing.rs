//! Phase 2.5 dogfood: prove the step-pattern engine handles a real
//! event-sourcing codegen `.feature` file verbatim.
//!
//! The fixture in `tests/fixtures/dogfood_event_sourcing.feature` is
//! a generic event-sourcing demo (`UserRegistered`, `RegisterUser`,
//! `UserList`, `Login`, `UserLoggedIn`) written in the same step
//! convention as real codegen exports. The pattern file in
//! `examples/event-sourcing.toml` maps each step type to an HTTP
//! shape on a mock event-store backend
//! (`POST /streams/{id}/events/{Event}`,
//! `POST /streams/{id}/commands/{Cmd}`, `GET /views/{View}`).
//! httpmock spins up an in-process server for the duration of the
//! test — no external backend needed.
//!
//! `--endpoint` / `--method` are passed as dummies because the CLI
//! still requires them today; they're never used because every
//! scenario fires `Action::HttpRequest` patterns before the
//! frontmatter fallback would run.

use assert_cmd::Command;
use httpmock::{Method, MockServer};
use predicates::prelude::*;
use serde_json::json;

const FIXTURE: &str = "tests/fixtures/dogfood_event_sourcing.feature";
const PATTERNS: &str = "examples/event-sourcing.toml";

#[test]
fn event_sourcing_codegen_runs_verbatim_against_mock_event_store() {
    let server = MockServer::start();

    // Setup endpoints — `Given the <Event> event has occurred on
    // stream "<id>":` fires POST against these. Bodies are ignored
    // by the assertions; status 200 with an empty body is enough.
    server.mock(|when, then| {
        when.method(Method::POST)
            .path("/streams/user-001/events/UserRegistered");
        then.status(200).json_body(json!({}));
    });
    server.mock(|when, then| {
        when.method(Method::POST)
            .path("/streams/user-002/events/UserRegistered");
        then.status(200).json_body(json!({}));
    });

    // View query — last response in scenario 1; its body must
    // contain (partial-match) the array in the Then doc string.
    server.mock(|when, then| {
        when.method(Method::GET).path("/views/UserList");
        then.status(200).json_body(json!([
            {
                "name": "Alice",
                "email": "alice@example.com",
                "userId": "user-001"
            },
            {
                "name": "Bob",
                "email": "bob@example.com",
                "userId": "user-002"
            }
        ]));
    });

    // RegisterUser command — returns the emitted event body, which
    // the `Then the UserRegistered event is emitted with:` doc string
    // is partial-matched against.
    server.mock(|when, then| {
        when.method(Method::POST)
            .path("/streams/user-001/commands/RegisterUser");
        then.status(200).json_body(json!({
            "name": "Alice",
            "email": "alice@example.com",
            "hashedPassword": "<<hashed>>"
        }));
    });

    // Login command — emits an empty `UserLoggedIn` event. The
    // Then assertion is `{}` so any response object passes
    // `json_contains` vacuously.
    server.mock(|when, then| {
        when.method(Method::POST)
            .path("/streams/user-001/commands/Login");
        then.status(200).json_body(json!({}));
    });

    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FIXTURE,
            "--base-url",
            &server.base_url(),
            "--patterns-file",
            PATTERNS,
            // Dummies: required by the CLI today, never used because
            // every scenario fires `HttpRequest` patterns first.
            "--endpoint",
            "/unused",
            "--method",
            "GET",
            "--no-report",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PASS]"))
        .stdout(predicate::str::contains("3 passed, 0 failed"));
}
