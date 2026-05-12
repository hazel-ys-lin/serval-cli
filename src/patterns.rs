//! Step-pattern engine for translating Gherkin step text into runner
//! actions.
//!
//! The Phase 2.1 refactor moved the previously inlined branches of
//! `runner::process_step` into this table. Phase 2.2 layered
//! user-defined patterns on top from a TOML file. Phase 2.3 lets
//! patterns *fire* HTTP requests directly via `Action::HttpRequest`,
//! turning a scenario from "one frontmatter-driven request" into a
//! multi-step state machine. Phase 2.4 will add `AssertBodyMatches`
//! for deep doc-string body comparison.
//!
//! Architecture (per project_phase2_pivot memory):
//! - **Tier 1 (built-in)**: this module, ships with the binary,
//!   covers generic HTTP-shape Gherkin.
//! - **Tier 2 (user-defined)**: per-project / global
//!   `patterns.toml`, loaded by Phase 2.2.
//!
//! A pattern is `regex + keyword-type filter + action`. When the
//! regex matches the (placeholder-substituted) step text and the
//! step's keyword type matches the filter, the action runs against
//! the scenario's [`crate::runner::ScenarioContext`].

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;

use regex::{Captures, Regex};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::gherkin::ParsedStep;
use crate::runner::{HttpResponse, ScenarioContext, StatusMatcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordType {
    Context, // Given
    Action,  // When
    Outcome, // Then
}

impl KeywordType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "Context" => Some(Self::Context),
            "Action" => Some(Self::Action),
            "Outcome" => Some(Self::Outcome),
            _ => None,
        }
    }
}

/// Where a value comes from when a data-driven action fires.
///
/// Used by [`Action::HttpRequest::body_from`] today; future
/// data-driven action variants (header values, query params, …) can
/// reuse it.
#[derive(Debug, Clone)]
pub enum ValueSource {
    /// Read from a named regex capture group on the step text. The
    /// pattern's `regex` must declare the group via `(?P<name>…)`.
    MatchGroup(String),
    /// Use the step's doc-string. Parsed as JSON if possible;
    /// otherwise wrapped as a `Value::String`.
    DocString,
    /// A constant baked into the pattern definition. No
    /// templating / substitution at Phase 2.3.
    Literal(Value),
}

/// What to do when a pattern's regex matches a step's text and the
/// keyword filter (if any) accepts it.
#[derive(Debug, Clone)]
pub enum Action {
    /// Scan the step text for the first 3-digit number in 100..600
    /// and set `expected_status`.
    AssertStatusFromTextScan,

    /// Find the first single- or double-quoted substring in the step
    /// text and push it to `expected_body_contains`.
    AssertBodyContainsFromQuotedScan,

    /// Whitespace-tokenise the step text, locate the literal word
    /// `header`, take the following token as the key and the final
    /// token as the value, and insert into `request_headers`.
    SetHeaderFromWordScan,

    /// Same word-based extraction with `param` / `parameter` as the
    /// anchor word — populates `query_params`.
    SetQueryParamFromWordScan,

    /// The step text mentions a body cue (handled by the matching
    /// pattern). If `request_body` is still empty, try to parse an
    /// inline JSON payload starting at the first `{`; otherwise,
    /// when the example row is an object, clone the row in as the
    /// body.
    SetRequestBodyFromTextOrExampleData,

    /// Parse the step's doc-string as JSON and assign to
    /// `request_body`. No-op when the step has no doc-string or it
    /// fails to parse as JSON. Used by built-in `Given` / `When`
    /// patterns to drive request payloads from triple-quoted blocks.
    SetRequestBodyFromDocString,

    /// Parse the step's doc-string as JSON and assign to
    /// `expected_body`, which is then deep-matched against the
    /// response body at end-of-scenario. No-op when no doc-string or
    /// parsing fails. Used by the built-in `Then` pattern.
    AssertBodyMatches,

    /// Set `expected_status` to a closed range `[min, max]`. Used
    /// by user patterns that need to assert "any 4xx" without
    /// pinning to a specific status code (e.g. failure-mode steps
    /// in event-sourcing codegen Gherkin).
    AssertExpectedStatusInRange { min: i16, max: i16 },

    /// Read a named regex capture group on the step text and push
    /// the captured substring to `expected_body_contains`. Used
    /// for patterns like `Then the operation fails with: <msg>`
    /// where the failure message is a regex capture.
    AssertBodyContainsFromMatchGroup { group: String },

    /// Fire an HTTP request and append the response to
    /// [`ScenarioContext::responses`]. `endpoint_template` may
    /// reference named regex capture groups from the matching
    /// pattern as `{{name}}`; unknown names expand to empty.
    HttpRequest {
        method: String,
        endpoint_template: String,
        body_from: Option<ValueSource>,
    },
}

#[derive(Debug)]
pub struct StepPattern {
    pub regex: Regex,
    /// `None` = pattern fires regardless of step keyword. `Some` =
    /// only fires for matching `Given` / `When` / `Then`.
    pub keyword_type: Option<KeywordType>,
    /// Actions to fire in order when this pattern matches. A single
    /// step text + regex match can drive multiple effects — e.g. a
    /// failure-mode `Then` step asserts both a status range and a
    /// body-contains substring from the same capture.
    pub actions: Vec<Action>,
}

/// Replace `<key>` placeholders in `text` with values from
/// `example_data`. Shared between the runner (step text / endpoint
/// substitution) and pattern actions (doc-string body / assertion
/// JSON).
pub fn substitute_placeholders(text: &str, example_data: &Value) -> String {
    let mut result = text.to_string();
    if let Some(obj) = example_data.as_object() {
        for (key, value) in obj {
            let placeholder = format!("<{key}>");
            let replacement = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => value.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    result
}

/// Resolve the user `patterns.toml` path. Resolution order:
/// 1. `$SERVAL_PATTERNS_FILE` if set.
/// 2. `$HOME/.serval/patterns.toml` otherwise.
pub fn default_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("SERVAL_PATTERNS_FILE") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME")
        .map_err(|_| Error::System("$HOME is not set; cannot locate ~/.serval/".into()))?;
    Ok(PathBuf::from(home).join(".serval").join("patterns.toml"))
}

/// Load and parse a `patterns.toml` file into a list of patterns.
/// Missing file returns an empty list (first-run UX, matching
/// `config::load`).
pub fn load_from_file(path: &Path) -> Result<Vec<StepPattern>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::System(format!("read patterns {}: {e}", path.display())))?;
    let parsed: TomlPatterns = toml::from_str(&content)
        .map_err(|e| Error::Spec(format!("patterns {}: {e}", path.display())))?;
    parsed
        .pattern
        .into_iter()
        .map(toml_to_pattern)
        .collect::<Result<Vec<_>>>()
        .map_err(|e| match e {
            Error::Spec(msg) => Error::Spec(format!("patterns {}: {msg}", path.display())),
            other => other,
        })
}

#[derive(Deserialize)]
struct TomlPatterns {
    #[serde(default)]
    pattern: Vec<TomlPattern>,
}

#[derive(Deserialize)]
struct TomlPattern {
    regex: String,
    #[serde(default)]
    keyword_type: Option<String>,
    actions: Vec<TomlAction>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TomlAction {
    AssertStatusFromTextScan,
    AssertBodyContainsFromQuotedScan,
    SetHeaderFromWordScan,
    SetQueryParamFromWordScan,
    SetRequestBodyFromTextOrExampleData,
    SetRequestBodyFromDocString,
    AssertBodyMatches,
    AssertExpectedStatusInRange {
        min: i16,
        max: i16,
    },
    AssertBodyContainsFromMatchGroup {
        group: String,
    },
    HttpRequest {
        method: String,
        endpoint_template: String,
        #[serde(default)]
        body_from: Option<TomlValueSource>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TomlValueSource {
    MatchGroup { name: String },
    DocString,
    Literal { value: Value },
}

fn toml_to_pattern(t: TomlPattern) -> Result<StepPattern> {
    let regex = Regex::new(&t.regex)
        .map_err(|e| Error::Spec(format!("invalid regex {:?}: {e}", t.regex)))?;
    let keyword_type = match t.keyword_type.as_deref() {
        Some("Context") => Some(KeywordType::Context),
        Some("Action") => Some(KeywordType::Action),
        Some("Outcome") => Some(KeywordType::Outcome),
        Some(other) => {
            return Err(Error::Spec(format!(
                "invalid keyword_type {other:?} (expected Context / Action / Outcome)"
            )));
        }
        None => None,
    };
    if t.actions.is_empty() {
        return Err(Error::Spec(
            "pattern must declare at least one action in `actions = [...]`".into(),
        ));
    }
    let actions: Vec<Action> = t.actions.into_iter().map(toml_to_action).collect();
    Ok(StepPattern {
        regex,
        keyword_type,
        actions,
    })
}

fn toml_to_action(t: TomlAction) -> Action {
    match t {
        TomlAction::AssertStatusFromTextScan => Action::AssertStatusFromTextScan,
        TomlAction::AssertBodyContainsFromQuotedScan => Action::AssertBodyContainsFromQuotedScan,
        TomlAction::SetHeaderFromWordScan => Action::SetHeaderFromWordScan,
        TomlAction::SetQueryParamFromWordScan => Action::SetQueryParamFromWordScan,
        TomlAction::SetRequestBodyFromTextOrExampleData => {
            Action::SetRequestBodyFromTextOrExampleData
        }
        TomlAction::SetRequestBodyFromDocString => Action::SetRequestBodyFromDocString,
        TomlAction::AssertBodyMatches => Action::AssertBodyMatches,
        TomlAction::AssertExpectedStatusInRange { min, max } => {
            Action::AssertExpectedStatusInRange { min, max }
        }
        TomlAction::AssertBodyContainsFromMatchGroup { group } => {
            Action::AssertBodyContainsFromMatchGroup { group }
        }
        TomlAction::HttpRequest {
            method,
            endpoint_template,
            body_from,
        } => Action::HttpRequest {
            method,
            endpoint_template,
            body_from: body_from.map(toml_to_value_source),
        },
    }
}

fn toml_to_value_source(t: TomlValueSource) -> ValueSource {
    match t {
        TomlValueSource::MatchGroup { name } => ValueSource::MatchGroup(name),
        TomlValueSource::DocString => ValueSource::DocString,
        TomlValueSource::Literal { value } => ValueSource::Literal(value),
    }
}

/// Built-in patterns shipped inside the binary. Reproduces the
/// pre-refactor behaviour of `runner::process_step` exactly.
pub fn builtin_patterns() -> Vec<StepPattern> {
    let mk = |re: &str, kt: Option<KeywordType>, actions: Vec<Action>| StepPattern {
        regex: Regex::new(re).expect("built-in pattern regex must compile"),
        keyword_type: kt,
        actions,
    };

    vec![
        // Outcome-step status code scan ("status should be 200",
        // "expect 500 error", "the status code is 404"). The actual
        // extraction sweeps the whole text for any 3-digit number in
        // 100..600 — the regex just gates which Outcome steps even
        // try.
        mk(
            r"(?i)\b(?:status|code|expect)",
            Some(KeywordType::Outcome),
            vec![Action::AssertStatusFromTextScan],
        ),
        // Outcome-step body-contains assertion via quoted literal.
        mk(
            r"(?i)contains|should\s+have",
            Some(KeywordType::Outcome),
            vec![Action::AssertBodyContainsFromQuotedScan],
        ),
        // Header extraction (any keyword).
        mk(r"(?i)\bheader\b", None, vec![Action::SetHeaderFromWordScan]),
        // Query-param extraction (any keyword).
        mk(
            r"(?i)query\s+param(?:eter)?\b",
            None,
            vec![Action::SetQueryParamFromWordScan],
        ),
        // Doc-string body for Given / When: ordered before the
        // text/example-data fallback so a triple-quoted block always
        // wins over inline JSON or row data.
        mk(
            "^",
            Some(KeywordType::Context),
            vec![Action::SetRequestBodyFromDocString],
        ),
        mk(
            "^",
            Some(KeywordType::Action),
            vec![Action::SetRequestBodyFromDocString],
        ),
        // Doc-string deep-match for Then: parses the triple-quoted
        // block into `expected_body` for end-of-scenario validation.
        mk(
            "^",
            Some(KeywordType::Outcome),
            vec![Action::AssertBodyMatches],
        ),
        // Body cue fallback for steps that announce a body but did
        // not provide a doc string.
        mk(
            r"(?i)\b(?:request\s+(?:body|payload)|with\s+body)\b",
            None,
            vec![Action::SetRequestBodyFromTextOrExampleData],
        ),
    ]
}

/// Apply every pattern whose regex matches `text` and whose keyword
/// filter accepts `step`. Non-HTTP actions mutate `context` /
/// `example_data` directly; `Action::HttpRequest` fires through
/// `client` and appends the result to `context.responses`.
///
/// Returns `Err` only when an HTTP firing fails at the transport
/// level (DNS, timeout, connection refused). Assertion-style
/// failures stay as state on `context` and are checked at
/// end-of-scenario.
pub async fn apply(
    patterns: &[StepPattern],
    step: &ParsedStep,
    text: &str,
    context: &mut ScenarioContext,
    example_data: &Value,
    client: &Client,
    base_url: &str,
) -> Result<()> {
    let kt = KeywordType::from_str(&step.keyword_type);
    for pattern in patterns {
        if let Some(required) = pattern.keyword_type
            && kt != Some(required)
        {
            continue;
        }
        let Some(captures) = pattern.regex.captures(text) else {
            continue;
        };
        for action in &pattern.actions {
            execute_action(
                action,
                text,
                &captures,
                step,
                context,
                example_data,
                client,
                base_url,
            )
            .await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_action(
    action: &Action,
    text: &str,
    captures: &Captures<'_>,
    step: &ParsedStep,
    context: &mut ScenarioContext,
    example_data: &Value,
    client: &Client,
    base_url: &str,
) -> Result<()> {
    match action {
        Action::HttpRequest {
            method,
            endpoint_template,
            body_from,
        } => {
            fire_http_request(
                method,
                endpoint_template,
                body_from.as_ref(),
                captures,
                step,
                context,
                client,
                base_url,
            )
            .await
        }
        other => {
            execute_sync_action(other, text, captures, step, context, example_data);
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_sync_action(
    action: &Action,
    text: &str,
    captures: &Captures<'_>,
    step: &ParsedStep,
    context: &mut ScenarioContext,
    example_data: &Value,
) {
    match action {
        Action::AssertStatusFromTextScan => {
            if let Some(status) = scan_status_code(text) {
                context.expected_status = Some(StatusMatcher::Exact(status));
            }
        }
        Action::AssertExpectedStatusInRange { min, max } => {
            context.expected_status = Some(StatusMatcher::Range {
                min: *min,
                max: *max,
            });
        }
        Action::AssertBodyContainsFromMatchGroup { group } => {
            if let Some(m) = captures.name(group) {
                context.expected_body_contains.push(m.as_str().to_string());
            }
        }
        Action::AssertBodyContainsFromQuotedScan => {
            if let Some(s) = scan_first_quoted(text) {
                context.expected_body_contains.push(s);
            }
        }
        Action::SetHeaderFromWordScan => {
            word_scan_pair(text, &["header"], &mut context.request_headers);
        }
        Action::SetQueryParamFromWordScan => {
            word_scan_pair(text, &["param", "parameter"], &mut context.query_params);
        }
        Action::SetRequestBodyFromTextOrExampleData => {
            if context.request_body.is_some() {
                return;
            }
            if let Some(json_start) = text.find('{')
                && let Ok(json) = serde_json::from_str::<Value>(&text[json_start..])
            {
                context.request_body = Some(json);
                return;
            }
            if !example_data.is_null() && example_data.is_object() {
                context.request_body = Some(example_data.clone());
            }
        }
        Action::SetRequestBodyFromDocString => {
            if let Some(doc) = &step.doc_string {
                let substituted = substitute_placeholders(doc, example_data);
                if let Ok(json) = serde_json::from_str::<Value>(&substituted) {
                    context.request_body = Some(json);
                }
            }
        }
        Action::AssertBodyMatches => {
            if let Some(doc) = &step.doc_string {
                let substituted = substitute_placeholders(doc, example_data);
                if let Ok(json) = serde_json::from_str::<Value>(&substituted) {
                    context.expected_body = Some(json);
                }
            }
        }
        Action::HttpRequest { .. } => {
            debug_assert!(false, "execute_sync_action called for HttpRequest variant");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn fire_http_request(
    method: &str,
    endpoint_template: &str,
    body_from: Option<&ValueSource>,
    captures: &Captures<'_>,
    step: &ParsedStep,
    context: &mut ScenarioContext,
    client: &Client,
    base_url: &str,
) -> Result<()> {
    let endpoint = substitute_endpoint(endpoint_template, captures);
    let url = format!("{}{}", base_url.trim_end_matches('/'), endpoint);
    let parsed_method = parse_method(method)?;

    let mut req = client.request(parsed_method, &url);
    for (k, v) in &context.request_headers {
        req = req.header(k, v);
    }

    let body = body_from.and_then(|src| resolve_value_source(src, step, captures));
    if let Some(b) = &body {
        req = req.json(b);
    }

    let started = Instant::now();
    let response = req
        .send()
        .await
        .map_err(|e| Error::System(format!("HTTP request failed: {e}")))?;
    let duration_ms = started.elapsed().as_millis() as i64;
    let status = response.status().as_u16() as i16;
    let resp_body = response.json::<Value>().await.unwrap_or(Value::Null);

    context.responses.push(HttpResponse {
        method: method.to_string(),
        url,
        status,
        body: resp_body,
        duration_ms,
    });
    Ok(())
}

fn parse_method(method: &str) -> Result<reqwest::Method> {
    use reqwest::Method;
    match method.to_uppercase().as_str() {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        "DELETE" => Ok(Method::DELETE),
        "PATCH" => Ok(Method::PATCH),
        "HEAD" => Ok(Method::HEAD),
        "OPTIONS" => Ok(Method::OPTIONS),
        _ => Err(Error::Spec(format!("Unsupported HTTP method: {method}"))),
    }
}

fn substitute_endpoint(template: &str, captures: &Captures<'_>) -> String {
    static PLACEHOLDER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{\{(\w+)\}\}").expect("hardcoded regex must compile"));
    PLACEHOLDER
        .replace_all(template, |caps: &Captures| {
            let name = &caps[1];
            captures
                .name(name)
                .map(|m| m.as_str())
                .unwrap_or("")
                .to_string()
        })
        .into_owned()
}

fn resolve_value_source(
    src: &ValueSource,
    step: &ParsedStep,
    captures: &Captures<'_>,
) -> Option<Value> {
    match src {
        ValueSource::MatchGroup(name) => captures
            .name(name)
            .map(|m| Value::String(m.as_str().to_string())),
        ValueSource::DocString => step.doc_string.as_deref().map(|s| {
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.to_string()))
        }),
        ValueSource::Literal(v) => Some(v.clone()),
    }
}

// ---------- private helpers (formerly methods on TestRunner) ----------

fn scan_status_code(text: &str) -> Option<i16> {
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

fn scan_first_quoted(text: &str) -> Option<String> {
    let mut in_quote = false;
    let mut quote_char = '"';
    let mut buf = String::new();
    for c in text.chars() {
        if !in_quote && (c == '"' || c == '\'') {
            in_quote = true;
            quote_char = c;
        } else if in_quote && c == quote_char {
            return Some(buf);
        } else if in_quote {
            buf.push(c);
        }
    }
    None
}

fn word_scan_pair(
    text: &str,
    anchor_words: &[&str],
    bucket: &mut std::collections::HashMap<String, String>,
) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let Some(idx) = words.iter().position(|w| anchor_words.contains(w)) else {
        return;
    };
    let (Some(key), Some(value)) = (words.get(idx + 1), words.last()) else {
        return;
    };
    let key = key.trim_matches(|c| c == '\'' || c == '"');
    let value = value.trim_matches(|c| c == '\'' || c == '"');
    bucket.insert(key.to_string(), value.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome_step(text: &str) -> ParsedStep {
        ParsedStep {
            keyword: "Then".to_string(),
            keyword_type: "Outcome".to_string(),
            text: text.to_string(),
            doc_string: None,
            data_table: None,
        }
    }

    fn action_step(text: &str) -> ParsedStep {
        ParsedStep {
            keyword: "When".to_string(),
            keyword_type: "Action".to_string(),
            text: text.to_string(),
            doc_string: None,
            data_table: None,
        }
    }

    /// Wraps `apply` with a fake Client + bogus base URL — fine for
    /// tests that don't exercise `HttpRequest`. Tests that DO need
    /// HTTP firing should call `apply` directly with an httpmock URL.
    async fn apply_sync_only(
        patterns: &[StepPattern],
        step: &ParsedStep,
        ctx: &mut ScenarioContext,
        example_data: &Value,
    ) {
        let client = Client::new();
        apply(
            patterns,
            step,
            &step.text,
            ctx,
            example_data,
            &client,
            "http://0.0.0.0",
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn status_pattern_picks_first_in_range_number() {
        let patterns = builtin_patterns();
        for (input, expected) in [
            ("status should be 200", 200_i16),
            ("the status code is 404", 404),
            ("expect 500 error", 500),
        ] {
            let mut ctx = ScenarioContext::default();
            let step = outcome_step(input);
            apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
            assert_eq!(
                ctx.expected_status,
                Some(StatusMatcher::Exact(expected)),
                "input {input:?} should set expected_status = {expected}"
            );
        }
    }

    #[tokio::test]
    async fn status_pattern_skips_non_outcome_steps() {
        let patterns = builtin_patterns();
        let step = action_step("status should be 200");
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert!(ctx.expected_status.is_none());
    }

    #[tokio::test]
    async fn body_contains_pattern_extracts_quoted_string() {
        let patterns = builtin_patterns();
        let step = outcome_step(r#"the body should have "hello world""#);
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(ctx.expected_body_contains, vec!["hello world".to_string()]);
    }

    #[tokio::test]
    async fn header_pattern_inserts_key_value() {
        let patterns = builtin_patterns();
        let step = action_step("I set header X-Custom my-token");
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.request_headers.get("X-Custom").map(String::as_str),
            Some("my-token")
        );
    }

    #[tokio::test]
    async fn query_param_pattern_inserts_key_value() {
        let patterns = builtin_patterns();
        let step = action_step("I set query param page 2");
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(ctx.query_params.get("page").map(String::as_str), Some("2"));
    }

    #[tokio::test]
    async fn body_cue_falls_back_to_example_data() {
        let patterns = builtin_patterns();
        let step = action_step("I POST /users with body");
        let mut ctx = ScenarioContext::default();
        let example = serde_json::json!({"email": "a@b.c"});
        apply_sync_only(&patterns, &step, &mut ctx, &example).await;
        assert_eq!(ctx.request_body, Some(example));
    }

    #[tokio::test]
    async fn doc_string_on_when_step_sets_request_body() {
        let patterns = builtin_patterns();
        let mut step = action_step("I POST /users");
        step.doc_string = Some(r#"{"email": "alice@example.com"}"#.to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.request_body,
            Some(serde_json::json!({"email": "alice@example.com"}))
        );
    }

    #[tokio::test]
    async fn doc_string_on_then_step_sets_expected_body_not_request_body() {
        let patterns = builtin_patterns();
        let mut step = outcome_step("the response is");
        step.doc_string = Some(r#"{"id": 1, "name": "alice"}"#.to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.expected_body,
            Some(serde_json::json!({"id": 1, "name": "alice"}))
        );
        assert!(
            ctx.request_body.is_none(),
            "Then doc string should not pollute request_body"
        );
    }

    #[tokio::test]
    async fn doc_string_substitutes_placeholders_from_example_row() {
        let patterns = builtin_patterns();
        let mut step = action_step("I POST /users");
        step.doc_string = Some(r#"{"email": "<email>"}"#.to_string());
        let mut ctx = ScenarioContext::default();
        let example = serde_json::json!({"email": "bob@example.com"});
        apply_sync_only(&patterns, &step, &mut ctx, &example).await;
        assert_eq!(
            ctx.request_body,
            Some(serde_json::json!({"email": "bob@example.com"}))
        );
    }

    #[tokio::test]
    async fn doc_string_non_json_is_silently_ignored() {
        let patterns = builtin_patterns();
        let mut step = action_step("I POST /users");
        step.doc_string = Some("not json at all".to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert!(ctx.request_body.is_none());
    }

    #[tokio::test]
    async fn no_doc_string_means_doc_actions_no_op() {
        let patterns = builtin_patterns();
        let step = outcome_step("status should be 200");
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        // status pattern still fires
        assert_eq!(ctx.expected_status, Some(StatusMatcher::Exact(200)));
        // but no doc string → no expected_body
        assert!(ctx.expected_body.is_none());
    }

    #[tokio::test]
    async fn body_cue_does_not_overwrite_existing_body() {
        let patterns = builtin_patterns();
        let step = action_step("I POST /users with body");
        let pre_set = serde_json::json!({"prior": true});
        let mut ctx = ScenarioContext {
            request_body: Some(pre_set.clone()),
            ..Default::default()
        };
        let example = serde_json::json!({"email": "a@b.c"});
        apply_sync_only(&patterns, &step, &mut ctx, &example).await;
        assert_eq!(ctx.request_body, Some(pre_set));
    }

    #[tokio::test]
    async fn assert_expected_status_in_range_sets_matcher() {
        let pattern = StepPattern {
            regex: Regex::new(r"^").unwrap(),
            keyword_type: Some(KeywordType::Outcome),
            actions: vec![Action::AssertExpectedStatusInRange { min: 400, max: 499 }],
        };
        let step = outcome_step("the operation fails with: bad request");
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&[pattern], &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.expected_status,
            Some(StatusMatcher::Range { min: 400, max: 499 })
        );
    }

    #[tokio::test]
    async fn assert_body_contains_from_match_group_captures() {
        let pattern = StepPattern {
            regex: Regex::new(r"(?i)fails with:\s*(?P<msg>.+)").unwrap(),
            keyword_type: Some(KeywordType::Outcome),
            actions: vec![Action::AssertBodyContainsFromMatchGroup {
                group: "msg".to_string(),
            }],
        };
        let step = outcome_step("the operation fails with: 帳號名稱已存在");
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&[pattern], &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.expected_body_contains,
            vec!["帳號名稱已存在".to_string()]
        );
    }

    #[tokio::test]
    async fn multi_action_pattern_fires_each_action_in_order() {
        // Phase 2.6 schema: one regex match can trigger multiple
        // actions (operation-fails-with combines status range +
        // body contains).
        let pattern = StepPattern {
            regex: Regex::new(r"(?i)the operation fails with:\s*(?P<msg>.+)").unwrap(),
            keyword_type: Some(KeywordType::Outcome),
            actions: vec![
                Action::AssertExpectedStatusInRange { min: 400, max: 499 },
                Action::AssertBodyContainsFromMatchGroup {
                    group: "msg".to_string(),
                },
            ],
        };
        let step = outcome_step("the operation fails with: not found");
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&[pattern], &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.expected_status,
            Some(StatusMatcher::Range { min: 400, max: 499 })
        );
        assert_eq!(ctx.expected_body_contains, vec!["not found".to_string()]);
    }

    // ---- ValueSource + endpoint template helpers ----

    #[test]
    fn substitute_placeholders_replaces_angle_bracket_keys() {
        let example = serde_json::json!({
            "email": "test@example.com",
            "password": "secret123"
        });
        let out = substitute_placeholders("user <email> with password <password>", &example);
        assert_eq!(out, "user test@example.com with password secret123");
    }

    #[test]
    fn substitute_placeholders_leaves_unknown_keys_intact() {
        let example = serde_json::json!({"a": 1});
        let out = substitute_placeholders("hi <b>", &example);
        assert_eq!(out, "hi <b>");
    }

    #[test]
    fn substitute_placeholders_null_example_passes_through() {
        let out = substitute_placeholders("hello <name>", &Value::Null);
        assert_eq!(out, "hello <name>");
    }

    #[test]
    fn endpoint_template_substitutes_named_groups() {
        let re = Regex::new(r"^id=(?P<id>\w+)$").unwrap();
        let caps = re.captures("id=alice").unwrap();
        assert_eq!(substitute_endpoint("/users/{{id}}", &caps), "/users/alice");
        assert_eq!(
            substitute_endpoint("/{{id}}/posts/{{id}}", &caps),
            "/alice/posts/alice"
        );
    }

    #[test]
    fn endpoint_template_unknown_group_expands_to_empty() {
        let re = Regex::new(r"^id=(?P<id>\w+)$").unwrap();
        let caps = re.captures("id=alice").unwrap();
        assert_eq!(substitute_endpoint("/users/{{missing}}", &caps), "/users/");
    }

    #[test]
    fn endpoint_template_no_placeholders_passes_through() {
        let re = Regex::new(r"^foo$").unwrap();
        let caps = re.captures("foo").unwrap();
        assert_eq!(substitute_endpoint("/static/path", &caps), "/static/path");
    }

    #[test]
    fn value_source_match_group_resolves() {
        let re = Regex::new(r"name=(?P<name>\w+)").unwrap();
        let caps = re.captures("name=bob").unwrap();
        let step = action_step("");
        let resolved =
            resolve_value_source(&ValueSource::MatchGroup("name".to_string()), &step, &caps);
        assert_eq!(resolved, Some(Value::String("bob".to_string())));
    }

    #[test]
    fn value_source_match_group_missing_resolves_to_none() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let step = action_step("");
        let resolved =
            resolve_value_source(&ValueSource::MatchGroup("nope".to_string()), &step, &caps);
        assert!(resolved.is_none());
    }

    #[test]
    fn value_source_doc_string_parses_json() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"a":1}"#.to_string());
        let resolved = resolve_value_source(&ValueSource::DocString, &step, &caps);
        assert_eq!(resolved, Some(serde_json::json!({"a": 1})));
    }

    #[test]
    fn value_source_doc_string_falls_back_to_string_on_invalid_json() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some("not json".to_string());
        let resolved = resolve_value_source(&ValueSource::DocString, &step, &caps);
        assert_eq!(resolved, Some(Value::String("not json".to_string())));
    }

    #[test]
    fn value_source_literal_clones() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let step = action_step("");
        let v = serde_json::json!({"hardcoded": true});
        let resolved = resolve_value_source(&ValueSource::Literal(v.clone()), &step, &caps);
        assert_eq!(resolved, Some(v));
    }

    // ---- TOML loader tests ----

    fn write_patterns(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn load_from_file_missing_yields_empty_list() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.toml");
        let result = load_from_file(&missing).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn load_from_file_parses_valid_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "user.toml",
            r#"
[[pattern]]
regex = '(?i)\bexpects\s+\d{3}\s+OK\b'
keyword_type = "Outcome"
[[pattern.actions]]
type = "assert_status_from_text_scan"
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].keyword_type, Some(KeywordType::Outcome));
        assert!(matches!(
            patterns[0].actions.as_slice(),
            [Action::AssertStatusFromTextScan]
        ));
    }

    #[test]
    fn load_from_file_supports_all_action_variants() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "all.toml",
            r#"
[[pattern]]
regex = '\bA\b'
[[pattern.actions]]
type = "assert_status_from_text_scan"

[[pattern]]
regex = '\bB\b'
[[pattern.actions]]
type = "assert_body_contains_from_quoted_scan"

[[pattern]]
regex = '\bC\b'
[[pattern.actions]]
type = "set_header_from_word_scan"

[[pattern]]
regex = '\bD\b'
[[pattern.actions]]
type = "set_query_param_from_word_scan"

[[pattern]]
regex = '\bE\b'
[[pattern.actions]]
type = "set_request_body_from_text_or_example_data"

[[pattern]]
regex = '\bF\b'
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/x"

[[pattern]]
regex = '\bG\b'
[[pattern.actions]]
type = "set_request_body_from_doc_string"

[[pattern]]
regex = '\bH\b'
[[pattern.actions]]
type = "assert_body_matches"
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        assert_eq!(patterns.len(), 8);
        assert!(matches!(
            patterns[5].actions[0],
            Action::HttpRequest { ref method, .. } if method == "POST"
        ));
        assert!(matches!(
            patterns[6].actions[0],
            Action::SetRequestBodyFromDocString
        ));
        assert!(matches!(patterns[7].actions[0], Action::AssertBodyMatches));
    }

    #[test]
    fn load_from_file_parses_multi_action_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "multi.toml",
            r#"
[[pattern]]
regex = '(?i)the operation fails with:\s*(?P<msg>.+)'
keyword_type = "Outcome"
[[pattern.actions]]
type = "assert_expected_status_in_range"
min = 400
max = 499
[[pattern.actions]]
type = "assert_body_contains_from_match_group"
group = "msg"
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].actions.len(), 2);
        assert!(matches!(
            patterns[0].actions[0],
            Action::AssertExpectedStatusInRange { min: 400, max: 499 }
        ));
        match &patterns[0].actions[1] {
            Action::AssertBodyContainsFromMatchGroup { group } => assert_eq!(group, "msg"),
            other => panic!("expected AssertBodyContainsFromMatchGroup, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_rejects_empty_actions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "empty.toml",
            r#"
[[pattern]]
regex = '\bx\b'
actions = []
"#,
        );
        let err = load_from_file(&path).unwrap_err();
        assert!(
            err.to_string().contains("at least one action"),
            "expected empty-actions error, got {err}"
        );
    }

    #[test]
    fn load_from_file_parses_http_request_with_body_from_match_group() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "http.toml",
            r#"
[[pattern]]
regex = '(?i)create user "(?P<name>[^"]+)"'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/users"
body_from = { kind = "match_group", name = "name" }
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        assert_eq!(patterns.len(), 1);
        match &patterns[0].actions[0] {
            Action::HttpRequest {
                method,
                endpoint_template,
                body_from,
            } => {
                assert_eq!(method, "POST");
                assert_eq!(endpoint_template, "/users");
                assert!(matches!(body_from, Some(ValueSource::MatchGroup(s)) if s == "name"));
            }
            other => panic!("expected HttpRequest, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_http_request_with_body_from_literal() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "lit.toml",
            r#"
[[pattern]]
regex = '\bcreate\b'
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/x"
body_from = { kind = "literal", value = { hello = "world" } }
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        match &patterns[0].actions[0] {
            Action::HttpRequest {
                body_from: Some(ValueSource::Literal(v)),
                ..
            } => {
                assert_eq!(v, &serde_json::json!({"hello": "world"}));
            }
            other => panic!("expected HttpRequest with literal body_from, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_rejects_invalid_regex() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "bad-regex.toml",
            r#"
[[pattern]]
regex = '['
[[pattern.actions]]
type = "assert_status_from_text_scan"
"#,
        );
        let err = load_from_file(&path).unwrap_err();
        assert!(
            err.to_string().contains("invalid regex"),
            "expected regex error, got {err}"
        );
    }

    #[test]
    fn load_from_file_rejects_invalid_keyword_type() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "bad-kt.toml",
            r#"
[[pattern]]
regex = '\bx\b'
keyword_type = "Setup"
[[pattern.actions]]
type = "assert_status_from_text_scan"
"#,
        );
        let err = load_from_file(&path).unwrap_err();
        assert!(
            err.to_string().contains("invalid keyword_type"),
            "expected keyword_type error, got {err}"
        );
    }

    #[test]
    fn load_from_file_rejects_unknown_action_type() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "bad-action.toml",
            r#"
[[pattern]]
regex = '\bx\b'
[[pattern.actions]]
type = "lol_make_coffee"
"#,
        );
        let err = load_from_file(&path).unwrap_err();
        // serde error wrapped in Error::Spec
        assert!(matches!(err, Error::Spec(_)));
    }
}
