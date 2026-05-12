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
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            auth_token: None,
            custom_headers: HashMap::new(),
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
    pub request_time: time::OffsetDateTime,
}

/// Context built up from Gherkin steps before firing the request.
#[derive(Debug, Clone, Default)]
pub struct StepContext {
    pub request_body: Option<serde_json::Value>,
    pub request_headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub path_params: HashMap<String, String>,
    pub expected_status: Option<i16>,
    pub expected_body: Option<serde_json::Value>,
    pub expected_body_contains: Vec<String>,
    /// Data table from a step (used as setup data; not currently
    /// asserted against).
    pub setup_data: Option<Vec<serde_json::Value>>,
}

pub struct TestRunner {
    client: Client,
    config: TestConfig,
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

        Ok(Self { client, config })
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

        let mut context = StepContext {
            expected_status: example.expected_status_code,
            ..Default::default()
        };

        for step in &scenario.steps {
            self.process_step(&mut context, step, &example.data);
        }

        let result = self
            .execute_request(api, env, &context, &example.data)
            .await;

        let duration = start.elapsed().as_millis() as i64;

        match result {
            Ok((status, body)) => {
                let validation = self.validate_response(status, &body, &context);

                TestResult {
                    scenario_title: scenario.title.clone(),
                    example_index,
                    pass: validation.is_ok(),
                    error_message: validation.err(),
                    response_status: status,
                    response_data: Some(body),
                    request_duration_ms: duration,
                    request_time,
                }
            }
            Err(e) => TestResult {
                scenario_title: scenario.title.clone(),
                example_index,
                pass: false,
                error_message: Some(e.to_string()),
                response_status: 0,
                response_data: None,
                request_duration_ms: duration,
                request_time,
            },
        }
    }

    fn process_step(
        &self,
        context: &mut StepContext,
        step: &ParsedStep,
        example_data: &serde_json::Value,
    ) {
        let text = self.substitute_placeholders(&step.text, example_data);

        if let Some(doc_str) = &step.doc_string {
            let substituted_doc = self.substitute_placeholders(doc_str, example_data);
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&substituted_doc) {
                context.request_body = Some(json);
            }
        }

        if let Some(table_data) = &step.data_table {
            let processed_table: Vec<serde_json::Value> = table_data
                .iter()
                .map(|row| {
                    let row_str = row.to_string();
                    let substituted = self.substitute_placeholders(&row_str, example_data);
                    serde_json::from_str(&substituted).unwrap_or(row.clone())
                })
                .collect();
            context.setup_data = Some(processed_table);
        }

        if (text.contains("request body")
            || text.contains("request payload")
            || text.contains("with body"))
            && context.request_body.is_none()
        {
            if let Some(json_start) = text.find('{') {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text[json_start..]) {
                    context.request_body = Some(json);
                }
            } else {
                context.request_body = Some(example_data.clone());
            }
        }

        if text.contains("header") {
            self.parse_header(&text, context);
        }

        if text.contains("query param") || text.contains("query parameter") {
            self.parse_query_param(&text, context);
        }

        if step.keyword_type == "Outcome" {
            if let Some(status) = self.extract_status_code(&text) {
                context.expected_status = Some(status);
            }

            if (text.contains("contains") || text.contains("should have"))
                && let Some(pattern) = self.extract_quoted_string(&text)
            {
                context.expected_body_contains.push(pattern);
            }
        }
    }

    fn substitute_placeholders(&self, text: &str, example_data: &serde_json::Value) -> String {
        let mut result = text.to_string();

        if let Some(obj) = example_data.as_object() {
            for (key, value) in obj {
                let placeholder = format!("<{key}>");
                let replacement = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => value.to_string(),
                };
                result = result.replace(&placeholder, &replacement);
            }
        }

        result
    }

    fn build_request_body(
        &self,
        context: &StepContext,
        example_data: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        if let Some(body) = &context.request_body {
            let body_str = body.to_string();
            let substituted = self.substitute_placeholders(&body_str, example_data);
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
        context: &StepContext,
        example_data: &serde_json::Value,
    ) -> Result<(i16, serde_json::Value)> {
        let endpoint = self.substitute_placeholders(&api.endpoint, example_data);
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

        let response: Response = request
            .send()
            .await
            .map_err(|e| Error::System(format!("HTTP request failed: {e}")))?;

        let status = response.status().as_u16() as i16;
        let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);

        Ok((status, body))
    }

    fn validate_response(
        &self,
        status: i16,
        body: &serde_json::Value,
        context: &StepContext,
    ) -> std::result::Result<(), String> {
        if let Some(expected_status) = context.expected_status
            && status != expected_status
        {
            return Err(format!("Expected status {expected_status}, got {status}"));
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

    fn parse_header(&self, text: &str, context: &mut StepContext) {
        let words: Vec<&str> = text.split_whitespace().collect();
        if let Some(header_idx) = words.iter().position(|&w| w == "header")
            && let (Some(key), Some(value)) = (words.get(header_idx + 1), words.last())
        {
            let key = key.trim_matches(|c| c == '\'' || c == '"');
            let value = value.trim_matches(|c| c == '\'' || c == '"');
            context
                .request_headers
                .insert(key.to_string(), value.to_string());
        }
    }

    fn parse_query_param(&self, text: &str, context: &mut StepContext) {
        let words: Vec<&str> = text.split_whitespace().collect();
        if let Some(param_idx) = words.iter().position(|&w| w == "param" || w == "parameter")
            && let (Some(key), Some(value)) = (words.get(param_idx + 1), words.last())
        {
            let key = key.trim_matches(|c| c == '\'' || c == '"');
            let value = value.trim_matches(|c| c == '\'' || c == '"');
            context
                .query_params
                .insert(key.to_string(), value.to_string());
        }
    }

    fn extract_status_code(&self, text: &str) -> Option<i16> {
        let words: Vec<&str> = text.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if let Ok(status) = word.parse::<i16>()
                && (100..600).contains(&status)
            {
                return Some(status);
            }
            if (*word == "status" || *word == "code")
                && let Some(next) = words.get(i + 1)
                && let Ok(status) = next.parse::<i16>()
                && (100..600).contains(&status)
            {
                return Some(status);
            }
        }
        None
    }

    fn extract_quoted_string(&self, text: &str) -> Option<String> {
        let mut in_quote = false;
        let mut quote_char = '"';
        let mut result = String::new();

        for c in text.chars() {
            if !in_quote && (c == '"' || c == '\'') {
                in_quote = true;
                quote_char = c;
            } else if in_quote && c == quote_char {
                return Some(result);
            } else if in_quote {
                result.push(c);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_placeholders() {
        let runner = TestRunner::new().unwrap();
        let example = serde_json::json!({
            "email": "test@example.com",
            "password": "secret123"
        });

        let result =
            runner.substitute_placeholders("user <email> with password <password>", &example);
        assert_eq!(result, "user test@example.com with password secret123");
    }

    #[test]
    fn test_extract_status_code() {
        let runner = TestRunner::new().unwrap();

        assert_eq!(
            runner.extract_status_code("status should be 200"),
            Some(200)
        );
        assert_eq!(
            runner.extract_status_code("the status code is 404"),
            Some(404)
        );
        assert_eq!(runner.extract_status_code("expect 500 error"), Some(500));
    }

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
