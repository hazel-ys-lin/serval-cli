//! Step-pattern engine for translating Gherkin step text into runner
//! actions.
//!
//! Today this module is consumed only by [`crate::runner`] through
//! the [`builtin_patterns`] table — which exactly preserves the
//! behaviour the runner had before this refactor (no functional
//! change at Phase 2.1). Phase 2.2 will layer user-defined patterns
//! on top from a TOML file; Phase 2.3 will let actions fire HTTP
//! requests directly (multi-step orchestration); Phase 2.4 will add
//! `AssertBodyMatches` for deep doc-string body comparison.
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
//! the scenario's [`crate::runner::StepContext`].

use regex::Regex;
use serde_json::Value;

use crate::gherkin::ParsedStep;
use crate::runner::StepContext;

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

/// What to do when a pattern's regex matches a step's text and the
/// keyword filter (if any) accepts it.
///
/// At Phase 2.1 every variant is a faithful re-expression of the
/// pre-refactor `runner::process_step` branches. Phase 2.2+ will
/// add data-driven variants (`HttpRequest`, `AssertBodyMatches`,
/// generic capture-group readers, …) that user patterns can target.
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
}

pub struct StepPattern {
    pub regex: Regex,
    /// `None` = pattern fires regardless of step keyword. `Some` =
    /// only fires for matching `Given` / `When` / `Then`.
    pub keyword_type: Option<KeywordType>,
    pub action: Action,
}

/// Built-in patterns shipped inside the binary. Reproduces the
/// pre-refactor behaviour of `runner::process_step` exactly.
pub fn builtin_patterns() -> Vec<StepPattern> {
    let mk = |re: &str, kt: Option<KeywordType>, action: Action| StepPattern {
        regex: Regex::new(re).expect("built-in pattern regex must compile"),
        keyword_type: kt,
        action,
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
            Action::AssertStatusFromTextScan,
        ),
        // Outcome-step body-contains assertion via quoted literal.
        mk(
            r"(?i)contains|should\s+have",
            Some(KeywordType::Outcome),
            Action::AssertBodyContainsFromQuotedScan,
        ),
        // Header extraction (any keyword).
        mk(r"(?i)\bheader\b", None, Action::SetHeaderFromWordScan),
        // Query-param extraction (any keyword).
        mk(
            r"(?i)query\s+param(?:eter)?\b",
            None,
            Action::SetQueryParamFromWordScan,
        ),
        // Body cue fallback for steps that announce a body but did
        // not provide a doc string.
        mk(
            r"(?i)\b(?:request\s+(?:body|payload)|with\s+body)\b",
            None,
            Action::SetRequestBodyFromTextOrExampleData,
        ),
    ]
}

/// Apply every pattern whose regex matches `text` and whose keyword
/// filter accepts `step`. Actions fire against `context` /
/// `example_data` directly.
pub fn apply(
    patterns: &[StepPattern],
    step: &ParsedStep,
    text: &str,
    context: &mut StepContext,
    example_data: &Value,
) {
    let kt = KeywordType::from_str(&step.keyword_type);
    for pattern in patterns {
        if let Some(required) = pattern.keyword_type
            && kt != Some(required)
        {
            continue;
        }
        if !pattern.regex.is_match(text) {
            continue;
        }
        execute_action(&pattern.action, text, context, example_data);
    }
}

fn execute_action(action: &Action, text: &str, context: &mut StepContext, example_data: &Value) {
    match action {
        Action::AssertStatusFromTextScan => {
            if let Some(status) = scan_status_code(text) {
                context.expected_status = Some(status);
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

    #[test]
    fn status_pattern_picks_first_in_range_number() {
        let patterns = builtin_patterns();
        for (input, expected) in [
            ("status should be 200", 200_i16),
            ("the status code is 404", 404),
            ("expect 500 error", 500),
        ] {
            let mut ctx = StepContext::default();
            let step = outcome_step(input);
            apply(&patterns, &step, &step.text, &mut ctx, &Value::Null);
            assert_eq!(
                ctx.expected_status,
                Some(expected),
                "input {input:?} should set expected_status = {expected}"
            );
        }
    }

    #[test]
    fn status_pattern_skips_non_outcome_steps() {
        let patterns = builtin_patterns();
        let step = action_step("status should be 200");
        let mut ctx = StepContext::default();
        apply(&patterns, &step, &step.text, &mut ctx, &Value::Null);
        assert!(ctx.expected_status.is_none());
    }

    #[test]
    fn body_contains_pattern_extracts_quoted_string() {
        let patterns = builtin_patterns();
        let step = outcome_step(r#"the body should have "hello world""#);
        let mut ctx = StepContext::default();
        apply(&patterns, &step, &step.text, &mut ctx, &Value::Null);
        assert_eq!(ctx.expected_body_contains, vec!["hello world".to_string()]);
    }

    #[test]
    fn header_pattern_inserts_key_value() {
        let patterns = builtin_patterns();
        let step = action_step("I set header X-Custom my-token");
        let mut ctx = StepContext::default();
        apply(&patterns, &step, &step.text, &mut ctx, &Value::Null);
        assert_eq!(
            ctx.request_headers.get("X-Custom").map(String::as_str),
            Some("my-token")
        );
    }

    #[test]
    fn query_param_pattern_inserts_key_value() {
        let patterns = builtin_patterns();
        let step = action_step("I set query param page 2");
        let mut ctx = StepContext::default();
        apply(&patterns, &step, &step.text, &mut ctx, &Value::Null);
        assert_eq!(ctx.query_params.get("page").map(String::as_str), Some("2"));
    }

    #[test]
    fn body_cue_falls_back_to_example_data() {
        let patterns = builtin_patterns();
        let step = action_step("I POST /users with body");
        let mut ctx = StepContext::default();
        let example = serde_json::json!({"email": "a@b.c"});
        apply(&patterns, &step, &step.text, &mut ctx, &example);
        assert_eq!(ctx.request_body, Some(example));
    }

    #[test]
    fn body_cue_does_not_overwrite_existing_body() {
        let patterns = builtin_patterns();
        let step = action_step("I POST /users with body");
        let pre_set = serde_json::json!({"prior": true});
        let mut ctx = StepContext {
            request_body: Some(pre_set.clone()),
            ..Default::default()
        };
        let example = serde_json::json!({"email": "a@b.c"});
        apply(&patterns, &step, &step.text, &mut ctx, &example);
        assert_eq!(ctx.request_body, Some(pre_set));
    }
}
