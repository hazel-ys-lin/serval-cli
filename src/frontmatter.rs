//! Optional YAML frontmatter at the top of a `.feature` file.
//!
//! Convention (from the spec-as-code design):
//!
//! ```yaml
//! ---
//! api:
//!   path: /api/orders
//!   method: POST
//!   collection: orders         # optional
//! implements:                  # optional
//!   - src/handlers/order.rs::create_order
//! ---
//! ```
//!
//! Followed by standard Gherkin. [`split`] returns
//! `(None, original_content)` if no `---` opener appears on the
//! first line, so callers can transparently accept files with or
//! without frontmatter.

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct Frontmatter {
    pub api: Option<ApiFrontmatter>,
    #[serde(default)]
    pub implements: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiFrontmatter {
    pub path: String,
    pub method: String,
    pub collection: Option<String>,
}

/// Split a `.feature`-shaped string into
/// `(Option<Frontmatter>, body)`. The body slice points into the
/// original input (no allocation). Frontmatter is optional: input
/// that does not begin with a `---` line passes through with
/// `Frontmatter = None`.
///
/// Returns [`Error::Spec`] when a `---` opener is present but no
/// matching closing `---` is found, or when the YAML between them
/// fails to deserialize.
pub fn split(content: &str) -> Result<(Option<Frontmatter>, &str)> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok((None, content));
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Ok((None, content));
    }

    let mut consumed = first.len();
    let mut yaml_buf = String::new();
    let mut found_close = false;

    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            consumed += line.len();
            found_close = true;
            break;
        }
        yaml_buf.push_str(line);
        consumed += line.len();
    }

    if !found_close {
        return Err(Error::Spec(
            "frontmatter: opening `---` found but no closing `---`".to_string(),
        ));
    }

    let frontmatter: Frontmatter = serde_yml::from_str(&yaml_buf)
        .map_err(|e| Error::Spec(format!("frontmatter YAML: {e}")))?;

    Ok((Some(frontmatter), &content[consumed..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_frontmatter_returns_none_and_passes_content_through() {
        let content = "Feature: hello\n  Scenario: ok\n";
        let (fm, body) = split(content).expect("should parse");
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn parses_minimal_api_frontmatter() {
        let content = "---\napi:\n  path: /api/foo\n  method: GET\n---\nFeature: hello\n";
        let (fm, body) = split(content).expect("should parse");
        let fm = fm.expect("frontmatter present");
        let api = fm.api.expect("api block present");
        assert_eq!(api.path, "/api/foo");
        assert_eq!(api.method, "GET");
        assert!(api.collection.is_none());
        assert!(fm.implements.is_none());
        assert_eq!(body, "Feature: hello\n");
    }

    #[test]
    fn parses_optional_fields() {
        let content = "\
---
api:
  path: /api/orders
  method: POST
  collection: orders
implements:
  - src/handlers/order.rs::create_order
  - src/services/validation.rs
---
Feature: foo
";
        let (fm, _body) = split(content).expect("should parse");
        let fm = fm.expect("frontmatter present");
        let api = fm.api.expect("api block present");
        assert_eq!(api.collection.as_deref(), Some("orders"));
        let implements = fm.implements.expect("implements present");
        assert_eq!(implements.len(), 2);
    }

    #[test]
    fn unclosed_frontmatter_errors() {
        let content = "---\napi:\n  path: /x\n  method: GET\nFeature: hello\n";
        assert!(split(content).is_err());
    }

    #[test]
    fn empty_input_returns_none() {
        let (fm, body) = split("").expect("should parse");
        assert!(fm.is_none());
        assert_eq!(body, "");
    }
}
