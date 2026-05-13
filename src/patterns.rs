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

use std::collections::HashMap;
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
    /// Look up a named scenario variable previously captured by an
    /// `HttpRequest` action's `capture_response` field. Returns
    /// `None` if the variable is not set.
    Variable(String),
    /// Reshape the step's doc-string before using it as a body.
    /// Resolution stacks three layers on the parsed top-level JSON
    /// object:
    /// 1. Apply `rename` (old key → new key) on the doc-string.
    /// 2. Lay `defaults` underneath — doc-string keys win on
    ///    collision.
    /// 3. Stamp `overrides` on top — `overrides` keys win even when
    ///    the doc-string supplies the same key.
    ///
    /// Targets codegen Gherkin whose body shape disagrees with the
    /// real backend: a Gherkin step that emits `{"username": "x"}`
    /// can be mapped to v2's `{"account": "x"}` with rename, and
    /// missing required fields (`organization`, `position`, ...) can
    /// be filled from `defaults`. `overrides` covers the harder case
    /// where a Gherkin literal value conflicts with a backend
    /// validator (e.g. Gherkin's `"password": "pass1234"` is 8 chars
    /// while v2 requires > 8 — override with a longer test password
    /// pattern-wide).
    ///
    /// Returns `None` if the step has no doc-string or the doc-
    /// string fails to parse as a JSON object.
    DocStringTemplate {
        rename: HashMap<String, String>,
        defaults: Value,
        overrides: Value,
    },
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

    /// Like `AssertBodyMatches`, but scope the deep-match to a
    /// sub-document of the response selected by a JSON pointer
    /// (RFC 6901). Closes the wire-shape gap when the codegen
    /// Gherkin doc-string is a bare collection but the backend
    /// wraps it (`{users: [...]}` from v2's `GET /users/list`
    /// vs Gherkin's `[...]`). Overwrites whatever the built-in
    /// `AssertBodyMatches` set on the same step — user patterns
    /// fire after built-ins, so the scoped form wins on collision.
    AssertBodyMatchesAt { pointer: String },

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
    /// [`ScenarioContext::responses`]. `endpoint_template` /
    /// `headers` values may reference regex capture groups as
    /// `{{name}}` or scenario variables as `{{$name}}`; unknown
    /// names expand to empty. `capture_response` evaluates each
    /// JSON-pointer expression against the response body and
    /// stores the result in `ScenarioContext.variables` under
    /// the given key for later steps.
    HttpRequest {
        method: String,
        endpoint_template: String,
        /// Boxed so the enum variant size stays under
        /// `clippy::large_enum_variant` — `DocStringTemplate`
        /// carries three heap-pointer fields and bumps the bare
        /// variant past the threshold.
        body_from: Option<Box<ValueSource>>,
        headers: HashMap<String, String>,
        capture_response: HashMap<String, String>,
        /// If non-empty, the response status must appear in this
        /// list — otherwise `fire_http_request` returns a Spec
        /// error and the scenario aborts at that step. Empty list
        /// (default) accepts any status, preserving pre-3.5
        /// behaviour where seed POSTs silently swallow errors.
        ///
        /// Targets cross-scenario seed idempotency: a `Given the
        /// AccountCreated event ...` pattern firing `POST
        /// /users/create` against a stateful backend gets 201 on
        /// fresh DB and 409 when the user pre-exists. Listing
        /// both (`accepted_status = [201, 409]`) treats either as
        /// "seed in place" and lets the scenario continue.
        accepted_status: Vec<i16>,
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

// Transient deserialization shape — converted to `Action` (which
// boxes the heavy `body_from`) immediately after `toml::from_str`.
// Boxing here would just add allocations during parsing without any
// runtime benefit.
#[allow(clippy::large_enum_variant)]
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
    AssertBodyMatchesAt {
        pointer: String,
    },
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
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        capture_response: HashMap<String, String>,
        #[serde(default)]
        accepted_status: Vec<i16>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TomlValueSource {
    MatchGroup {
        name: String,
    },
    DocString,
    Literal {
        value: Value,
    },
    Variable {
        name: String,
    },
    DocStringTemplate {
        #[serde(default)]
        rename: HashMap<String, String>,
        #[serde(default)]
        defaults: Value,
        #[serde(default)]
        overrides: Value,
    },
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
        TomlAction::AssertBodyMatchesAt { pointer } => Action::AssertBodyMatchesAt { pointer },
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
            headers,
            capture_response,
            accepted_status,
        } => Action::HttpRequest {
            method,
            endpoint_template,
            body_from: body_from.map(|b| Box::new(toml_to_value_source(b))),
            headers,
            capture_response,
            accepted_status,
        },
    }
}

fn toml_to_value_source(t: TomlValueSource) -> ValueSource {
    match t {
        TomlValueSource::MatchGroup { name } => ValueSource::MatchGroup(name),
        TomlValueSource::DocString => ValueSource::DocString,
        TomlValueSource::Literal { value } => ValueSource::Literal(value),
        TomlValueSource::Variable { name } => ValueSource::Variable(name),
        TomlValueSource::DocStringTemplate {
            rename,
            defaults,
            overrides,
        } => ValueSource::DocStringTemplate {
            rename,
            defaults,
            overrides,
        },
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

/// Per-call cross-cutting state for [`apply`]. Bundles the HTTP
/// client, base URL, and global headers (from `--header` / TestConfig)
/// so the function signature stays manageable as Phase 3.0 adds
/// header support.
pub struct ApplyContext<'a> {
    pub client: &'a Client,
    pub base_url: &'a str,
    pub global_headers: &'a HashMap<String, String>,
}

/// Apply every pattern whose regex matches `text` and whose keyword
/// filter accepts `step`. Non-HTTP actions mutate `context` /
/// `example_data` directly; `Action::HttpRequest` fires through
/// `apply_ctx.client` and appends the result to `context.responses`,
/// optionally capturing response fields into `context.variables`.
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
    apply_ctx: &ApplyContext<'_>,
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
                apply_ctx,
            )
            .await?;
        }
    }
    Ok(())
}

async fn execute_action(
    action: &Action,
    text: &str,
    captures: &Captures<'_>,
    step: &ParsedStep,
    context: &mut ScenarioContext,
    example_data: &Value,
    apply_ctx: &ApplyContext<'_>,
) -> Result<()> {
    match action {
        Action::HttpRequest {
            method,
            endpoint_template,
            body_from,
            headers,
            capture_response,
            accepted_status,
        } => {
            fire_http_request(
                method,
                endpoint_template,
                body_from.as_deref(),
                headers,
                capture_response,
                accepted_status,
                captures,
                step,
                context,
                apply_ctx,
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
                if let Ok(json) = serde_json::from_str::<Value>(&substituted)
                    && !is_vacuous_expected_body(&json)
                {
                    context.expected_body = Some(json);
                }
            }
        }
        Action::AssertBodyMatchesAt { pointer } => {
            if let Some(doc) = &step.doc_string {
                let substituted = substitute_placeholders(doc, example_data);
                if let Ok(json) = serde_json::from_str::<Value>(&substituted)
                    && !is_vacuous_expected_body(&json)
                {
                    context.expected_body = Some(json);
                    context.expected_body_pointer = Some(pointer.clone());
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
    pattern_headers: &HashMap<String, String>,
    capture_response: &HashMap<String, String>,
    accepted_status: &[i16],
    captures: &Captures<'_>,
    step: &ParsedStep,
    context: &mut ScenarioContext,
    apply_ctx: &ApplyContext<'_>,
) -> Result<()> {
    let endpoint = substitute_template(endpoint_template, captures, &context.variables);
    let url = format!("{}{}", apply_ctx.base_url.trim_end_matches('/'), endpoint);
    let parsed_method = parse_method(method)?;

    let mut req = apply_ctx.client.request(parsed_method, &url);
    // Global headers from TestConfig (CLI --header / programmatic).
    for (k, v) in apply_ctx.global_headers {
        req = req.header(k, v);
    }
    // Headers accumulated on the scenario context via
    // SetHeaderFromWordScan etc.
    for (k, v) in &context.request_headers {
        req = req.header(k, v);
    }
    // Per-pattern headers — values support `{{regex}}` / `{{$var}}`
    // substitution so an authenticated pattern can pull a token from
    // an earlier login step's captured response.
    for (k, v) in pattern_headers {
        let resolved = substitute_template(v, captures, &context.variables);
        req = req.header(k, resolved);
    }

    let body =
        body_from.and_then(|src| resolve_value_source(src, step, captures, &context.variables));
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

    // Phase 3.5: per-action accepted-status enforcement. If the
    // pattern declared a non-empty `accepted_status` list and the
    // response status isn't in it, abort the scenario at this step
    // with a clear message — guards against silently swallowed
    // setup errors (e.g. a seed POST returning 500 mid-scenario).
    // The captured response is still pushed onto `context.responses`
    // so post-mortem reports can show what came back.
    let status_accepted = accepted_status.is_empty() || accepted_status.contains(&status);

    // Phase 3.0: variable capture. Each `name = "/json/pointer"` entry
    // pulls a value out of the response body via RFC 6901 and stores
    // it on the scenario context under `name` for subsequent steps to
    // reference via `{{$name}}` or `ValueSource::Variable`.
    for (var_name, pointer) in capture_response {
        if let Some(captured) = resp_body.pointer(pointer) {
            context.variables.insert(var_name.clone(), captured.clone());
        }
    }

    context.responses.push(HttpResponse {
        method: method.to_string(),
        url: url.clone(),
        status,
        body: resp_body,
        duration_ms,
    });

    if !status_accepted {
        return Err(Error::Spec(format!(
            "HTTP {method} {url} returned status {status}, not in accepted_status {accepted_status:?}"
        )));
    }
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

/// Substitute `{{regex_group}}` and `{{$variable}}` placeholders in
/// `template`. Capture-group references read from the matching
/// pattern's `Captures`; variable references read from the scenario
/// context's accumulated variables (populated by `capture_response`).
/// Unknown names expand to empty.
fn substitute_template(
    template: &str,
    captures: &Captures<'_>,
    variables: &HashMap<String, Value>,
) -> String {
    static PLACEHOLDER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{\{(\$)?(\w+)\}\}").expect("hardcoded regex must compile"));
    PLACEHOLDER
        .replace_all(template, |caps: &Captures| {
            let is_var = caps.get(1).is_some();
            let name = &caps[2];
            if is_var {
                match variables.get(name) {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                }
            } else {
                captures
                    .name(name)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default()
            }
        })
        .into_owned()
}

fn resolve_value_source(
    src: &ValueSource,
    step: &ParsedStep,
    captures: &Captures<'_>,
    variables: &HashMap<String, Value>,
) -> Option<Value> {
    match src {
        ValueSource::MatchGroup(name) => captures
            .name(name)
            .map(|m| Value::String(m.as_str().to_string())),
        ValueSource::DocString => step.doc_string.as_deref().map(|s| {
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.to_string()))
        }),
        ValueSource::Literal(v) => Some(v.clone()),
        ValueSource::Variable(name) => variables.get(name).cloned(),
        ValueSource::DocStringTemplate {
            rename,
            defaults,
            overrides,
        } => {
            let doc = step.doc_string.as_deref()?;
            let parsed: Value = serde_json::from_str(doc).ok()?;
            let doc_obj = parsed.as_object()?;
            Some(apply_doc_string_template(
                doc_obj, rename, defaults, overrides,
            ))
        }
    }
}

/// Reshape a doc-string JSON object via `rename` (key remapping),
/// `defaults` (fallback fields), and `overrides` (forced fields).
/// Layers stack: defaults (bottom) ← renamed doc-string ← overrides
/// (top). Non-object `defaults` / `overrides` are treated as empty
/// — a safety net for malformed pattern config.
fn apply_doc_string_template(
    doc_obj: &serde_json::Map<String, Value>,
    rename: &HashMap<String, String>,
    defaults: &Value,
    overrides: &Value,
) -> Value {
    let mut renamed = serde_json::Map::with_capacity(doc_obj.len());
    for (k, v) in doc_obj {
        let mapped = rename.get(k).cloned().unwrap_or_else(|| k.clone());
        renamed.insert(mapped, v.clone());
    }
    let mut out = match defaults.as_object() {
        Some(d) => d.clone(),
        None => serde_json::Map::new(),
    };
    for (k, v) in renamed {
        out.insert(k, v);
    }
    if let Some(o) = overrides.as_object() {
        for (k, v) in o {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

// ---------- private helpers (formerly methods on TestRunner) ----------

/// True when a parsed doc-string body is too empty to carry an
/// assertion. Codegen Gherkin commonly writes `Then ... emitted with:
/// {}` as documentation of "an event of this shape is emitted" with
/// no field requirements; deep-partial matching `{}` against any
/// response body would pass trivially and produce false-PASS
/// reports. Treating it as a no-op lets strict mode catch the
/// scenario as having no assertion. Empty arrays are intentionally
/// not flagged here — `[]` plausibly means "expect empty list"; that
/// gap needs an explicit assert-equals action, not a vacuous-PASS
/// silencer.
fn is_vacuous_expected_body(v: &Value) -> bool {
    matches!(v, Value::Object(map) if map.is_empty())
}

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
        let headers: HashMap<String, String> = HashMap::new();
        let apply_ctx = ApplyContext {
            client: &client,
            base_url: "http://0.0.0.0",
            global_headers: &headers,
        };
        apply(patterns, step, &step.text, ctx, example_data, &apply_ctx)
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
    async fn empty_object_doc_string_on_then_does_not_set_expected_body() {
        // Phase 3.2: codegen Gherkin uses `Then ... emitted with: {}`
        // to mean "an event of this shape exists" without asserting
        // any field. Setting expected_body = {} would deep-partial
        // match any body trivially. The engine drops it so strict
        // mode catches the missing assertion.
        let patterns = builtin_patterns();
        let mut step = outcome_step("the response is");
        step.doc_string = Some("{}".to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert!(
            ctx.expected_body.is_none(),
            "empty object doc-string should leave expected_body unset; got {:?}",
            ctx.expected_body
        );
    }

    #[tokio::test]
    async fn empty_object_doc_string_around_status_keeps_status_assertion() {
        // The Outcome step text triggers two built-in patterns:
        // status scan AND assert-body-matches. The status assertion
        // should still fire even though the doc-string body is
        // dropped as vacuous.
        let patterns = builtin_patterns();
        let mut step = outcome_step("status should be 200");
        step.doc_string = Some("{}".to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(ctx.expected_status, Some(StatusMatcher::Exact(200)));
        assert!(ctx.expected_body.is_none());
    }

    #[tokio::test]
    async fn assert_body_matches_at_sets_expected_body_and_pointer() {
        // Phase 3.4: scopes the deep-match to a JSON sub-document.
        // Mirrors the v2 AccountList case where Gherkin's `Then
        // the view returns: [...]` should compare against the
        // response's `/users` sub-array rather than the whole body.
        let pattern = StepPattern {
            regex: Regex::new(r"(?i)the view returns").unwrap(),
            keyword_type: Some(KeywordType::Outcome),
            actions: vec![Action::AssertBodyMatchesAt {
                pointer: "/users".to_string(),
            }],
        };
        let mut step = outcome_step("the view returns");
        step.doc_string = Some(r#"[{"name": "Alice"}]"#.to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&[pattern], &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.expected_body,
            Some(serde_json::json!([{"name": "Alice"}]))
        );
        assert_eq!(ctx.expected_body_pointer.as_deref(), Some("/users"));
    }

    #[tokio::test]
    async fn assert_body_matches_at_overwrites_builtin_body_match() {
        // Pattern order: built-in `AssertBodyMatches` fires first
        // (no pointer), then user `AssertBodyMatchesAt` reassigns
        // with a pointer. The scoped form should win, otherwise
        // users couldn't escape the whole-body partial-match default.
        let patterns: Vec<StepPattern> = builtin_patterns()
            .into_iter()
            .chain(std::iter::once(StepPattern {
                regex: Regex::new(r"(?i)the view returns").unwrap(),
                keyword_type: Some(KeywordType::Outcome),
                actions: vec![Action::AssertBodyMatchesAt {
                    pointer: "/users".to_string(),
                }],
            }))
            .collect();
        let mut step = outcome_step("the view returns");
        step.doc_string = Some(r#"[{"name": "Alice"}]"#.to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(
            ctx.expected_body,
            Some(serde_json::json!([{"name": "Alice"}]))
        );
        assert_eq!(ctx.expected_body_pointer.as_deref(), Some("/users"));
    }

    #[tokio::test]
    async fn assert_body_matches_at_empty_object_doc_string_skipped() {
        // Same vacuous-{} rule as the plain AssertBodyMatches: an
        // empty doc-string body carries no real assertion regardless
        // of pointer.
        let pattern = StepPattern {
            regex: Regex::new(r"(?i)the view returns").unwrap(),
            keyword_type: Some(KeywordType::Outcome),
            actions: vec![Action::AssertBodyMatchesAt {
                pointer: "/users".to_string(),
            }],
        };
        let mut step = outcome_step("the view returns");
        step.doc_string = Some("{}".to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&[pattern], &step, &mut ctx, &Value::Null).await;
        assert!(ctx.expected_body.is_none());
        assert!(ctx.expected_body_pointer.is_none());
    }

    #[tokio::test]
    async fn empty_array_doc_string_still_sets_expected_body() {
        // Empty arrays are intentionally NOT flagged as vacuous —
        // `[]` may plausibly mean "expect empty list". A future
        // assert-equals action will tighten this; for now the
        // current partial-match behaviour is preserved.
        let patterns = builtin_patterns();
        let mut step = outcome_step("the view returns");
        step.doc_string = Some("[]".to_string());
        let mut ctx = ScenarioContext::default();
        apply_sync_only(&patterns, &step, &mut ctx, &Value::Null).await;
        assert_eq!(ctx.expected_body, Some(serde_json::json!([])));
    }

    #[test]
    fn is_vacuous_expected_body_only_flags_empty_object() {
        assert!(is_vacuous_expected_body(&serde_json::json!({})));
        assert!(!is_vacuous_expected_body(&serde_json::json!({"a": 1})));
        assert!(!is_vacuous_expected_body(&serde_json::json!([])));
        assert!(!is_vacuous_expected_body(&serde_json::json!([1, 2])));
        assert!(!is_vacuous_expected_body(&Value::Null));
        assert!(!is_vacuous_expected_body(&serde_json::json!("")));
        assert!(!is_vacuous_expected_body(&serde_json::json!(0)));
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

    fn empty_vars() -> HashMap<String, Value> {
        HashMap::new()
    }

    #[test]
    fn endpoint_template_substitutes_named_groups() {
        let re = Regex::new(r"^id=(?P<id>\w+)$").unwrap();
        let caps = re.captures("id=alice").unwrap();
        assert_eq!(
            substitute_template("/users/{{id}}", &caps, &empty_vars()),
            "/users/alice"
        );
        assert_eq!(
            substitute_template("/{{id}}/posts/{{id}}", &caps, &empty_vars()),
            "/alice/posts/alice"
        );
    }

    #[test]
    fn endpoint_template_unknown_group_expands_to_empty() {
        let re = Regex::new(r"^id=(?P<id>\w+)$").unwrap();
        let caps = re.captures("id=alice").unwrap();
        assert_eq!(
            substitute_template("/users/{{missing}}", &caps, &empty_vars()),
            "/users/"
        );
    }

    #[test]
    fn endpoint_template_no_placeholders_passes_through() {
        let re = Regex::new(r"^foo$").unwrap();
        let caps = re.captures("foo").unwrap();
        assert_eq!(
            substitute_template("/static/path", &caps, &empty_vars()),
            "/static/path"
        );
    }

    #[test]
    fn template_substitutes_scenario_variable() {
        let re = Regex::new(r"^x$").unwrap();
        let caps = re.captures("x").unwrap();
        let mut vars = HashMap::new();
        vars.insert("user_id".to_string(), Value::String("u-123".to_string()));
        assert_eq!(
            substitute_template("/users/{{$user_id}}", &caps, &vars),
            "/users/u-123"
        );
    }

    #[test]
    fn template_unknown_variable_expands_to_empty() {
        let re = Regex::new(r"^x$").unwrap();
        let caps = re.captures("x").unwrap();
        assert_eq!(
            substitute_template("/auth/{{$missing}}", &caps, &empty_vars()),
            "/auth/"
        );
    }

    #[test]
    fn template_variable_and_capture_in_same_string() {
        let re = Regex::new(r"^id=(?P<id>\w+)$").unwrap();
        let caps = re.captures("id=alice").unwrap();
        let mut vars = HashMap::new();
        vars.insert("tenant".to_string(), Value::String("acme".to_string()));
        assert_eq!(
            substitute_template("/{{$tenant}}/users/{{id}}", &caps, &vars),
            "/acme/users/alice"
        );
    }

    #[test]
    fn value_source_match_group_resolves() {
        let re = Regex::new(r"name=(?P<name>\w+)").unwrap();
        let caps = re.captures("name=bob").unwrap();
        let step = action_step("");
        let resolved = resolve_value_source(
            &ValueSource::MatchGroup("name".to_string()),
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(resolved, Some(Value::String("bob".to_string())));
    }

    #[test]
    fn value_source_match_group_missing_resolves_to_none() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let step = action_step("");
        let resolved = resolve_value_source(
            &ValueSource::MatchGroup("nope".to_string()),
            &step,
            &caps,
            &empty_vars(),
        );
        assert!(resolved.is_none());
    }

    #[test]
    fn value_source_doc_string_parses_json() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"a":1}"#.to_string());
        let resolved = resolve_value_source(&ValueSource::DocString, &step, &caps, &empty_vars());
        assert_eq!(resolved, Some(serde_json::json!({"a": 1})));
    }

    #[test]
    fn value_source_doc_string_falls_back_to_string_on_invalid_json() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some("not json".to_string());
        let resolved = resolve_value_source(&ValueSource::DocString, &step, &caps, &empty_vars());
        assert_eq!(resolved, Some(Value::String("not json".to_string())));
    }

    #[test]
    fn value_source_literal_clones() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let step = action_step("");
        let v = serde_json::json!({"hardcoded": true});
        let resolved = resolve_value_source(
            &ValueSource::Literal(v.clone()),
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(resolved, Some(v));
    }

    #[test]
    fn value_source_variable_resolves_when_present() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let step = action_step("");
        let mut vars = HashMap::new();
        vars.insert("token".to_string(), Value::String("abc.def".to_string()));
        let resolved = resolve_value_source(
            &ValueSource::Variable("token".to_string()),
            &step,
            &caps,
            &vars,
        );
        assert_eq!(resolved, Some(Value::String("abc.def".to_string())));
    }

    #[test]
    fn value_source_variable_missing_resolves_to_none() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let step = action_step("");
        let resolved = resolve_value_source(
            &ValueSource::Variable("nope".to_string()),
            &step,
            &caps,
            &empty_vars(),
        );
        assert!(resolved.is_none());
    }

    #[test]
    fn doc_string_template_renames_keys() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"username": "coach_wang", "password": "p"}"#.to_string());
        let mut rename = HashMap::new();
        rename.insert("username".to_string(), "account".to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename,
                defaults: Value::Null,
                overrides: Value::Null,
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(
            resolved,
            Some(serde_json::json!({"account": "coach_wang", "password": "p"}))
        );
    }

    #[test]
    fn doc_string_template_fills_missing_fields_from_defaults() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"name": "王教練", "password": "p"}"#.to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: serde_json::json!({
                    "organization": "v2-dogfood",
                    "position": "coach",
                    "roles": ["user_full"],
                }),
                overrides: Value::Null,
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(
            resolved,
            Some(serde_json::json!({
                "name": "王教練",
                "password": "p",
                "organization": "v2-dogfood",
                "position": "coach",
                "roles": ["user_full"],
            }))
        );
    }

    #[test]
    fn doc_string_template_doc_string_overrides_defaults() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"position": "captain"}"#.to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: serde_json::json!({"position": "coach"}),
                overrides: Value::Null,
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(resolved, Some(serde_json::json!({"position": "captain"})));
    }

    #[test]
    fn doc_string_template_rename_and_defaults_compose() {
        // Mirrors the v2 CreateAccount case: rename `username` →
        // `account`, fill `organization` / `position` / `roles` from
        // defaults, hash placeholder is implicit (defaults supply
        // `password` which doc-string overrides if present).
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(
            r#"{"name": "王教練", "remark": "U12", "password": "p", "username": "coach_wang"}"#
                .to_string(),
        );
        let mut rename = HashMap::new();
        rename.insert("username".to_string(), "account".to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename,
                defaults: serde_json::json!({
                    "organization": "v2-dogfood",
                    "position": "coach",
                    "roles": ["user_full"],
                }),
                overrides: Value::Null,
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(
            resolved,
            Some(serde_json::json!({
                "account": "coach_wang",
                "name": "王教練",
                "remark": "U12",
                "password": "p",
                "organization": "v2-dogfood",
                "position": "coach",
                "roles": ["user_full"],
            }))
        );
    }

    #[test]
    fn doc_string_template_no_doc_string_resolves_to_none() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let step = action_step("");
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: serde_json::json!({"a": 1}),
                overrides: Value::Null,
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert!(resolved.is_none());
    }

    #[test]
    fn doc_string_template_invalid_json_resolves_to_none() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some("not json at all".to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: Value::Null,
                overrides: Value::Null,
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert!(resolved.is_none());
    }

    #[test]
    fn doc_string_template_overrides_win_over_doc_string() {
        // Phase 3.3: `overrides` is the top merge layer — it wins
        // even when the doc-string supplies the same key. Mirrors the
        // v2 case where Gherkin's literal `pass1234` (8 chars) fails
        // a "> 8 chars" backend validator and must be replaced
        // pattern-wide.
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"password": "pass1234", "username": "coach"}"#.to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: Value::Null,
                overrides: serde_json::json!({"password": "Pass12345!"}),
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(
            resolved,
            Some(serde_json::json!({"password": "Pass12345!", "username": "coach"}))
        );
    }

    #[test]
    fn doc_string_template_overrides_win_over_defaults() {
        // overrides also beats defaults when both supply the same
        // key — guards the layering order.
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"name": "Alice"}"#.to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: serde_json::json!({"role": "user"}),
                overrides: serde_json::json!({"role": "admin"}),
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(
            resolved,
            Some(serde_json::json!({"name": "Alice", "role": "admin"}))
        );
    }

    #[test]
    fn doc_string_template_overrides_compose_with_rename_and_defaults() {
        // Full v2 Login shape: rename `username` -> `account`,
        // defaults supply the (unused here) extra fields, overrides
        // force a long-enough password.
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"username": "coach_wang", "password": "pass1234"}"#.to_string());
        let mut rename = HashMap::new();
        rename.insert("username".to_string(), "account".to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename,
                defaults: serde_json::json!({"placeholder": true}),
                overrides: serde_json::json!({"password": "Pass12345!"}),
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(
            resolved,
            Some(serde_json::json!({
                "account": "coach_wang",
                "password": "Pass12345!",
                "placeholder": true,
            }))
        );
    }

    #[test]
    fn doc_string_template_non_object_overrides_ignored() {
        // Malformed `overrides` (non-object) is treated as empty
        // rather than producing a panic / spec error. Matches the
        // existing tolerance for non-object `defaults`.
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some(r#"{"a": 1}"#.to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: Value::Null,
                overrides: serde_json::json!("not an object"),
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert_eq!(resolved, Some(serde_json::json!({"a": 1})));
    }

    #[test]
    fn doc_string_template_non_object_doc_string_resolves_to_none() {
        let re = Regex::new(r"x").unwrap();
        let caps = re.captures("x").unwrap();
        let mut step = action_step("");
        step.doc_string = Some("[1, 2, 3]".to_string());
        let resolved = resolve_value_source(
            &ValueSource::DocStringTemplate {
                rename: HashMap::new(),
                defaults: Value::Null,
                overrides: Value::Null,
            },
            &step,
            &caps,
            &empty_vars(),
        );
        assert!(resolved.is_none());
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
                ..
            } => {
                assert_eq!(method, "POST");
                assert_eq!(endpoint_template, "/users");
                match body_from.as_deref() {
                    Some(ValueSource::MatchGroup(s)) => assert_eq!(s, "name"),
                    other => panic!("expected MatchGroup, got {other:?}"),
                }
            }
            other => panic!("expected HttpRequest, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_http_request_with_capture_and_headers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "cap.toml",
            r#"
[[pattern]]
regex = '(?i)protected access'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/things"
headers = { Authorization = "Bearer {{$token}}", X-Tenant = "acme" }
capture_response = { id = "/id", trace = "/meta/trace_id" }
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        match &patterns[0].actions[0] {
            Action::HttpRequest {
                headers,
                capture_response,
                ..
            } => {
                assert_eq!(
                    headers.get("Authorization").map(String::as_str),
                    Some("Bearer {{$token}}")
                );
                assert_eq!(headers.get("X-Tenant").map(String::as_str), Some("acme"));
                assert_eq!(capture_response.get("id").map(String::as_str), Some("/id"));
                assert_eq!(
                    capture_response.get("trace").map(String::as_str),
                    Some("/meta/trace_id")
                );
            }
            other => panic!("expected HttpRequest, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_http_request_with_accepted_status() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "accepted.toml",
            r#"
[[pattern]]
regex = '(?i)the (?P<event>\w+) event has occurred'
keyword_type = "Context"
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/users/create"
accepted_status = [201, 409]
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        match &patterns[0].actions[0] {
            Action::HttpRequest {
                accepted_status, ..
            } => {
                assert_eq!(accepted_status, &vec![201_i16, 409]);
            }
            other => panic!("expected HttpRequest, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_http_request_without_accepted_status_defaults_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "no-accepted.toml",
            r#"
[[pattern]]
regex = '\bF\b'
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/x"
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        match &patterns[0].actions[0] {
            Action::HttpRequest {
                accepted_status, ..
            } => {
                assert!(accepted_status.is_empty());
            }
            other => panic!("expected HttpRequest, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_assert_body_matches_at() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "matches-at.toml",
            r#"
[[pattern]]
regex = '(?i)the (?P<view>\w+) view returns'
keyword_type = "Outcome"
[[pattern.actions]]
type = "assert_body_matches_at"
pointer = "/users"
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        assert_eq!(patterns.len(), 1);
        match &patterns[0].actions[0] {
            Action::AssertBodyMatchesAt { pointer } => assert_eq!(pointer, "/users"),
            other => panic!("expected AssertBodyMatchesAt, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_http_request_with_body_from_doc_string_template() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "tpl.toml",
            r#"
[[pattern]]
regex = '(?i)\w+ sends CreateAccount on stream "(?P<stream>[^"]+)"'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/users/create"
body_from = { kind = "doc_string_template", rename = { username = "account" }, defaults = { organization = "v2-dogfood", position = "coach", roles = ["user_full"] } }
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        let Action::HttpRequest { body_from, .. } = &patterns[0].actions[0] else {
            panic!("expected HttpRequest, got {:?}", &patterns[0].actions[0]);
        };
        match body_from.as_deref() {
            Some(ValueSource::DocStringTemplate {
                rename,
                defaults,
                overrides,
            }) => {
                assert_eq!(rename.get("username").map(String::as_str), Some("account"));
                assert_eq!(
                    defaults,
                    &serde_json::json!({
                        "organization": "v2-dogfood",
                        "position": "coach",
                        "roles": ["user_full"],
                    })
                );
                assert!(overrides.is_null());
            }
            other => panic!("expected DocStringTemplate, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_doc_string_template_with_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "tpl-overrides.toml",
            r#"
[[pattern]]
regex = '(?i)Anonymous sends Login'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/auth/login"
body_from = { kind = "doc_string_template", rename = { username = "account" }, overrides = { password = "Pass12345!" } }
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        let Action::HttpRequest { body_from, .. } = &patterns[0].actions[0] else {
            panic!("expected HttpRequest, got {:?}", &patterns[0].actions[0]);
        };
        match body_from.as_deref() {
            Some(ValueSource::DocStringTemplate {
                rename,
                defaults,
                overrides,
            }) => {
                assert_eq!(rename.get("username").map(String::as_str), Some("account"));
                assert!(defaults.is_null());
                assert_eq!(overrides, &serde_json::json!({"password": "Pass12345!"}));
            }
            other => panic!("expected DocStringTemplate, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_parses_doc_string_template_with_only_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_patterns(
            tmp.path(),
            "rename-only.toml",
            r#"
[[pattern]]
regex = '(?i)Anonymous sends Login'
keyword_type = "Action"
[[pattern.actions]]
type = "http_request"
method = "POST"
endpoint_template = "/auth/login"
body_from = { kind = "doc_string_template", rename = { username = "account" } }
"#,
        );
        let patterns = load_from_file(&path).unwrap();
        let Action::HttpRequest { body_from, .. } = &patterns[0].actions[0] else {
            panic!("expected HttpRequest, got {:?}", &patterns[0].actions[0]);
        };
        match body_from.as_deref() {
            Some(ValueSource::DocStringTemplate {
                rename,
                defaults,
                overrides,
            }) => {
                assert_eq!(rename.len(), 1);
                assert_eq!(rename.get("username").map(String::as_str), Some("account"));
                assert!(defaults.is_null());
                assert!(overrides.is_null());
            }
            other => panic!("expected DocStringTemplate, got {other:?}"),
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
        let Action::HttpRequest { body_from, .. } = &patterns[0].actions[0] else {
            panic!("expected HttpRequest, got {:?}", &patterns[0].actions[0]);
        };
        match body_from.as_deref() {
            Some(ValueSource::Literal(v)) => {
                assert_eq!(v, &serde_json::json!({"hello": "world"}));
            }
            other => panic!("expected literal body_from, got {other:?}"),
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
