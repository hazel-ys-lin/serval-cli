//! HTTP test runner — executes a parsed Gherkin scenario against an
//! HTTP target and reports pass/fail per example.
//!
//! Entry point: [`TestRunner::run_scenario`]. Inputs are decoupled
//! from any database / entity layer:
//! - [`ParsedScenario`] from [`crate::gherkin`] carries steps +
//!   examples directly (no JSON round-trip).
//! - [`ApiSpec`] is the API endpoint + HTTP method.
//! - [`EnvSpec`] is the target base URL.
//!
//! Phase 2.3 makes the runner multi-step: a scenario can fire
//! multiple HTTP requests via `Action::HttpRequest` patterns,
//! recorded as [`HttpResponse`] entries on the
//! [`ScenarioContext`]. Validation runs against the *last* response.
//! Scenarios that never trigger an `HttpRequest` pattern fall back
//! to the pre-2.3 behaviour — a single frontmatter-driven request
//! at end-of-scenario — so existing specs keep working unchanged.
//!
//! Test-assertion failures (status / body mismatch) come back as
//! `pass: false` on a [`TestResult`] — they are **not** `Result::Err`.
//! `Result::Err` is reserved for spec / infrastructure problems
//! (see [`crate::error::Error`]).

use reqwest::{Client, Method, Response};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::gherkin::{ParsedExample, ParsedScenario, ParsedStep};
use crate::patterns::{self, StepPattern};

/// Plain DTO replacing v2's `Api` entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSpec {
    pub endpoint: String,
    pub http_method: String,
}

/// Plain DTO replacing v2's `Environment` entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSpec {
    pub base_url: String,
}

/// Configuration for test execution.
#[derive(Debug, Clone)]
pub struct TestConfig {
    pub timeout: Duration,
    pub auth_token: Option<String>,
    pub custom_headers: HashMap<String, String>,
    /// When `false` (strict mode, the default), a scenario that
    /// reaches end-of-run without setting any assertion
    /// (`expected_status`, `expected_body`, `expected_body_contains`)
    /// is marked as failed. Set to `true` via the CLI flag
    /// `--allow-no-assertions` to opt out — useful for pure
    /// fire-and-forget scenarios.
    pub allow_no_assertions: bool,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            auth_token: None,
            custom_headers: HashMap::new(),
            allow_no_assertions: false,
        }
    }
}

/// How a scenario asserts against the response status. Patterns set
/// [`ScenarioContext::expected_status`] to one of these; the runner
/// then validates the actual status accordingly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusMatcher {
    /// Status must equal `n` exactly.
    Exact(i16),
    /// Status must fall in the closed range `[min, max]`.
    Range { min: i16, max: i16 },
}

impl StatusMatcher {
    pub fn matches(&self, status: i16) -> bool {
        match self {
            Self::Exact(s) => *s == status,
            Self::Range { min, max } => (*min..=*max).contains(&status),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Exact(s) => s.to_string(),
            Self::Range { min, max } => format!("{min}..={max}"),
        }
    }
}

/// Result of a single test execution (one Examples row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub scenario_title: String,
    pub example_index: i32,
    pub pass: bool,
    pub error_message: Option<String>,
    pub response_status: i16,
    pub response_data: Option<serde_json::Value>,
    pub request_duration_ms: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub request_time: time::OffsetDateTime,
}

/// A single HTTP exchange recorded during scenario execution. The
/// runner appends one per HTTP request fired — whether by an
/// `Action::HttpRequest` pattern firing mid-scenario or by the
/// implicit frontmatter-driven fallback at end-of-scenario.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub method: String,
    pub url: String,
    pub status: i16,
    pub body: serde_json::Value,
    pub duration_ms: i64,
}

/// Mutable state accumulated across a scenario's steps. Filled by
/// the step-pattern engine ([`patterns::apply`]) as each step is
/// processed; consumed at end-of-scenario for the implicit fallback
/// request and response validation.
#[derive(Debug, Clone, Default)]
pub struct ScenarioContext {
    pub request_body: Option<serde_json::Value>,
    pub request_headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub path_params: HashMap<String, String>,
    pub expected_status: Option<StatusMatcher>,
    pub expected_body: Option<serde_json::Value>,
    pub expected_body_contains: Vec<String>,
    /// Data table from a step (used as setup data; not currently
    /// asserted against).
    pub setup_data: Option<Vec<serde_json::Value>>,
    /// HTTP responses recorded by `Action::HttpRequest` patterns
    /// firing during step processing. If still empty after all
    /// steps run, the runner falls back to firing one implicit
    /// request driven by the spec's frontmatter `api.path` /
    /// `api.method` (backward compatible with Phase 2.2 specs).
    pub responses: Vec<HttpResponse>,
    /// Named scenario variables captured by `HttpRequest`
    /// actions' `capture_response` fields. Sticky across steps
    /// within a scenario; reset between scenarios. Referenced from
    /// later patterns via `{{$name}}` substitution or
    /// `ValueSource::Variable`.
    pub variables: HashMap<String, serde_json::Value>,
}

pub struct TestRunner {
    client: Client,
    config: TestConfig,
    patterns: Vec<StepPattern>,
}

impl TestRunner {
    pub fn new() -> Result<Self> {
        Self::with_config(TestConfig::default())
    }

    pub fn with_config(config: TestConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| Error::System(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            config,
            patterns: patterns::builtin_patterns(),
        })
    }

    /// Append additional patterns after the built-in ones. User-loaded
    /// patterns (from `~/.serval/patterns.toml` or `--patterns-file`)
    /// flow through here; both built-in and user patterns can match a
    /// single step.
    pub fn extend_patterns(mut self, extra: Vec<StepPattern>) -> Self {
        self.patterns.extend(extra);
        self
    }

    /// Run every example in `scenario` against `api` on `env`.
    ///
    /// Concrete scenarios — those with `Given/When/Then` steps but no
    /// `Examples:` table — run exactly once with `Value::Null` as the
    /// implicit row data. Placeholder substitution sees no fields to
    /// expand, so the step text passes through unchanged.
    pub async fn run_scenario(
        &self,
        scenario: &ParsedScenario,
        api: &ApiSpec,
        env: &EnvSpec,
    ) -> Result<Vec<TestResult>> {
        let examples: Cow<[ParsedExample]> = if scenario.examples.is_empty() {
            Cow::Owned(vec![ParsedExample {
                data: serde_json::Value::Null,
                expected_status_code: None,
            }])
        } else {
            Cow::Borrowed(&scenario.examples)
        };

        let mut results = Vec::with_capacity(examples.len());

        for (index, example) in examples.iter().enumerate() {
            let result = self
                .run_example(scenario, api, env, example, index as i32)
                .await;
            results.push(result);
        }

        Ok(results)
    }

    async fn run_example(
        &self,
        scenario: &ParsedScenario,
        api: &ApiSpec,
        env: &EnvSpec,
        example: &ParsedExample,
        example_index: i32,
    ) -> TestResult {
        let request_time = time::OffsetDateTime::now_utc();
        let start = Instant::now();

        let mut context = ScenarioContext {
            expected_status: example.expected_status_code.map(StatusMatcher::Exact),
            ..Default::default()
        };

        let mut step_failure: Option<String> = None;
        for step in &scenario.steps {
            if let Err(e) = self
                .process_step(&mut context, step, env, &example.data)
                .await
            {
                step_failure = Some(e.to_string());
                break;
            }
        }

        // Backward-compatible fallback: if no `Action::HttpRequest`
        // pattern fired during the scenario, run the implicit
        // frontmatter-driven request now. This preserves the pre-2.3
        // single-request behaviour for specs that don't use the new
        // pattern engine.
        if step_failure.is_none() && context.responses.is_empty() {
            match self
                .execute_request(api, env, &context, &example.data)
                .await
            {
                Ok(resp) => context.responses.push(resp),
                Err(e) => step_failure = Some(e.to_string()),
            }
        }

        let duration = start.elapsed().as_millis() as i64;

        if let Some(err) = step_failure {
            return TestResult {
                scenario_title: scenario.title.clone(),
                example_index,
                pass: false,
                error_message: Some(err),
                response_status: 0,
                response_data: None,
                request_duration_ms: duration,
                request_time,
            };
        }

        let last = context
            .responses
            .last()
            .expect("responses non-empty after fallback")
            .clone();
        let validation = self.validate_response(last.status, &last.body, &context);

        TestResult {
            scenario_title: scenario.title.clone(),
            example_index,
            pass: validation.is_ok(),
            error_message: validation.err(),
            response_status: last.status,
            response_data: Some(last.body),
            request_duration_ms: duration,
            request_time,
        }
    }

    async fn process_step(
        &self,
        context: &mut ScenarioContext,
        step: &ParsedStep,
        env: &EnvSpec,
        example_data: &serde_json::Value,
    ) -> Result<()> {
        let text = patterns::substitute_placeholders(&step.text, example_data);

        // Data table: stash as setup data for later assertions.
        // (Doc strings are now pattern-driven via
        // `SetRequestBodyFromDocString` / `AssertBodyMatches`; data
        // tables stay here because they have no pattern action yet.)
        if let Some(table_data) = &step.data_table {
            let processed_table: Vec<serde_json::Value> = table_data
                .iter()
                .map(|row| {
                    let row_str = row.to_string();
                    let substituted = patterns::substitute_placeholders(&row_str, example_data);
                    serde_json::from_str(&substituted).unwrap_or(row.clone())
                })
                .collect();
            context.setup_data = Some(processed_table);
        }

        // Text-driven actions: status codes, headers, query params,
        // body cues, HTTP firing, doc-string body / assertion.
        // Pattern engine handles all of it.
        let apply_ctx = patterns::ApplyContext {
            client: &self.client,
            base_url: &env.base_url,
            global_headers: &self.config.custom_headers,
        };
        patterns::apply(
            &self.patterns,
            step,
            &text,
            context,
            example_data,
            &apply_ctx,
        )
        .await
    }

    fn build_request_body(
        &self,
        context: &ScenarioContext,
        example_data: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        if let Some(body) = &context.request_body {
            let body_str = body.to_string();
            let substituted = patterns::substitute_placeholders(&body_str, example_data);
            serde_json::from_str(&substituted).ok()
        } else if !example_data.is_null() && example_data.is_object() {
            let mut body = example_data.clone();
            if let Some(obj) = body.as_object_mut() {
                obj.remove("expected_status");
                obj.remove("expected_status_code");
                obj.remove("expected_response_body");
            }
            Some(body)
        } else {
            None
        }
    }

    async fn execute_request(
        &self,
        api: &ApiSpec,
        env: &EnvSpec,
        context: &ScenarioContext,
        example_data: &serde_json::Value,
    ) -> Result<HttpResponse> {
        let endpoint = patterns::substitute_placeholders(&api.endpoint, example_data);
        let mut url = format!("{}{}", env.base_url.trim_end_matches('/'), endpoint);

        if !context.query_params.is_empty() {
            let params: Vec<String> = context
                .query_params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            url = format!("{}?{}", url, params.join("&"));
        }

        let method = match api.http_method.to_uppercase().as_str() {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            "PATCH" => Method::PATCH,
            "HEAD" => Method::HEAD,
            "OPTIONS" => Method::OPTIONS,
            _ => {
                return Err(Error::Spec(format!(
                    "Unsupported HTTP method: {}",
                    api.http_method
                )));
            }
        };

        let mut request = self.client.request(method.clone(), &url);

        if let Some(token) = &self.config.auth_token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }

        for (key, value) in &self.config.custom_headers {
            request = request.header(key, value);
        }

        for (key, value) in &context.request_headers {
            request = request.header(key, value);
        }

        if matches!(method, Method::POST | Method::PUT | Method::PATCH)
            && let Some(body) = self.build_request_body(context, example_data)
        {
            request = request.json(&body);
        }

        let started = Instant::now();
        let response: Response = request
            .send()
            .await
            .map_err(|e| Error::System(format!("HTTP request failed: {e}")))?;
        let duration_ms = started.elapsed().as_millis() as i64;
        let status = response.status().as_u16() as i16;
        let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);

        Ok(HttpResponse {
            method: api.http_method.to_uppercase(),
            url,
            status,
            body,
            duration_ms,
        })
    }

    fn validate_response(
        &self,
        status: i16,
        body: &serde_json::Value,
        context: &ScenarioContext,
    ) -> std::result::Result<(), String> {
        if !self.config.allow_no_assertions && !has_any_assertion(context) {
            return Err("scenario ran without setting any assertion; pass \
                 `--allow-no-assertions` to override or add a pattern that \
                 fires `AssertExpectedStatusInRange` / `AssertBodyContains*` \
                 / `AssertBodyMatches`"
                .to_string());
        }

        if let Some(expected) = &context.expected_status
            && !expected.matches(status)
        {
            return Err(format!(
                "Expected status {}, got {status}",
                expected.describe()
            ));
        }

        for pattern in &context.expected_body_contains {
            let body_str = body.to_string();
            if !body_str.contains(pattern) {
                return Err(format!(
                    "Response body does not contain expected pattern: {pattern}"
                ));
            }
        }

        if let Some(expected_body) = &context.expected_body
            && !expected_body.is_null()
            && !self.json_contains(body, expected_body)
        {
            return Err(format!(
                "Response body does not match expected. Expected: {expected_body}, Got: {body}"
            ));
        }

        Ok(())
    }

    fn json_contains(&self, actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
        match (actual, expected) {
            (serde_json::Value::Object(actual_obj), serde_json::Value::Object(expected_obj)) => {
                expected_obj.iter().all(|(key, expected_value)| {
                    actual_obj
                        .get(key)
                        .map(|actual_value| self.json_contains(actual_value, expected_value))
                        .unwrap_or(false)
                })
            }
            (serde_json::Value::Array(actual_arr), serde_json::Value::Array(expected_arr)) => {
                expected_arr.iter().all(|expected_item| {
                    actual_arr
                        .iter()
                        .any(|actual_item| self.json_contains(actual_item, expected_item))
                })
            }
            _ => actual == expected,
        }
    }
}

/// True when the scenario context has at least one assertion set.
/// Used by strict mode (the default) to flag vacuous PASSes —
/// scenarios that run their HTTP calls but never set an
/// `expected_*` field, which would otherwise return Ok vacuously.
fn has_any_assertion(context: &ScenarioContext) -> bool {
    context.expected_status.is_some()
        || context.expected_body.is_some()
        || !context.expected_body_contains.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: `substitute_placeholders` lives in `patterns` and is
    // covered by `patterns::tests::substitute_placeholders_*`.
    // Status-code extraction is covered by
    // `patterns::tests::status_pattern_picks_first_in_range_number`.

    #[test]
    fn test_json_contains() {
        let runner = TestRunner::new().unwrap();

        let actual = serde_json::json!({
            "id": 1,
            "name": "test",
            "extra": "field"
        });

        let expected = serde_json::json!({
            "id": 1,
            "name": "test"
        });

        assert!(runner.json_contains(&actual, &expected));

        let not_expected = serde_json::json!({
            "id": 2
        });
        assert!(!runner.json_contains(&actual, &not_expected));
    }
}
