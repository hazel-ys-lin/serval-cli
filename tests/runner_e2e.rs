//! End-to-end tests for `TestRunner::run_scenario` against a real
//! HTTP server (a `httpmock` fake), covering both code paths:
//! concrete scenarios (no `Examples:`) and `Scenario Outline:` with
//! a populated `Examples:` table.

use httpmock::{Method, MockServer};
use serde_json::json;
use serval_cli::gherkin::{ParsedExample, ParsedScenario, ParsedStep};
use serval_cli::runner::{ApiSpec, EnvSpec, TestRunner};

fn step(keyword: &str, keyword_type: &str, text: &str) -> ParsedStep {
    ParsedStep {
        keyword: keyword.to_string(),
        keyword_type: keyword_type.to_string(),
        text: text.to_string(),
        doc_string: None,
        data_table: None,
    }
}

#[tokio::test]
async fn concrete_scenario_runs_exactly_once() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET).path("/health");
            then.status(200).json_body(json!({"status": "ok"}));
        })
        .await;

    let scenario = ParsedScenario {
        title: "Service is healthy".to_string(),
        description: None,
        tags: vec![],
        steps: vec![
            step("Given", "Context", "the service is deployed"),
            step("When", "Action", "I query /health"),
            step("Then", "Outcome", "status should be 200"),
        ],
        examples: vec![], // empty — concrete scenario
    };

    let api = ApiSpec {
        endpoint: "/health".to_string(),
        http_method: "GET".to_string(),
    };
    let env = EnvSpec {
        base_url: server.base_url(),
    };

    let runner = TestRunner::new().expect("runner builds");
    let results = runner
        .run_scenario(&scenario, &api, &env)
        .await
        .expect("scenario runs");

    assert_eq!(
        results.len(),
        1,
        "concrete scenario must yield 1 TestResult"
    );
    assert!(
        results[0].pass,
        "expected pass, got: {:?}",
        results[0].error_message
    );
    assert_eq!(results[0].response_status, 200);
    mock.assert_hits_async(1).await;
}

#[tokio::test]
async fn data_driven_scenario_runs_once_per_example() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET).path("/users");
            then.status(200).json_body(json!([]));
        })
        .await;

    let scenario = ParsedScenario {
        title: "Listing users".to_string(),
        description: None,
        tags: vec![],
        steps: vec![step("Then", "Outcome", "the response status should be 200")],
        examples: vec![
            ParsedExample {
                data: json!({"page": 1}),
                expected_status_code: Some(200),
            },
            ParsedExample {
                data: json!({"page": 2}),
                expected_status_code: Some(200),
            },
            ParsedExample {
                data: json!({"page": 3}),
                expected_status_code: Some(200),
            },
        ],
    };

    let api = ApiSpec {
        endpoint: "/users".to_string(),
        http_method: "GET".to_string(),
    };
    let env = EnvSpec {
        base_url: server.base_url(),
    };

    let runner = TestRunner::new().expect("runner builds");
    let results = runner
        .run_scenario(&scenario, &api, &env)
        .await
        .expect("scenario runs");

    assert_eq!(results.len(), 3, "one TestResult per example");
    assert!(results.iter().all(|r| r.pass), "all examples must pass");
    mock.assert_hits_async(3).await;
}
