//! Gherkin feature-file parser.
//!
//! Wraps the `gherkin` crate's AST into plainer DTOs
//! ([`ParsedFeature`] / [`ParsedScenario`] / [`ParsedStep`] /
//! [`ParsedExample`]) that downstream code can consume without learning
//! the gherkin crate's types. Cell values in data tables and Examples
//! rows go through `parse_cell_value` for light type inference
//! (int / float / bool / null / JSON literal / string).
//!
//! Entry point: [`GherkinService::parse`].

use gherkin::{Feature, GherkinEnv, StepType};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Parsed Gherkin step with optional doc string and data table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedStep {
    pub keyword: String,
    pub keyword_type: String,
    pub text: String,
    /// Multi-line doc string (for JSON bodies, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_string: Option<String>,
    /// Data table embedded in the step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_table: Option<Vec<serde_json::Value>>,
}

/// Parsed example row (test data + expected status code).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedExample {
    pub data: serde_json::Value,
    pub expected_status_code: Option<i16>,
}

/// Parsed scenario from a Gherkin feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedScenario {
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub steps: Vec<ParsedStep>,
    pub examples: Vec<ParsedExample>,
}

/// Result of parsing a Gherkin feature file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedFeature {
    pub name: String,
    pub description: Option<String>,
    /// Background steps that run before each scenario.
    pub background_steps: Vec<ParsedStep>,
    pub scenarios: Vec<ParsedScenario>,
}

pub struct GherkinService;

impl GherkinService {
    /// Parse a Gherkin feature file's textual content into a
    /// [`ParsedFeature`]. Returns [`Error::Spec`] on parse failure.
    pub fn parse(feature_code: &str) -> Result<ParsedFeature> {
        let feature = Feature::parse(feature_code, GherkinEnv::default())
            .map_err(|e| Error::Spec(format!("Gherkin parse error: {e}")))?;

        let background_steps = feature
            .background
            .as_ref()
            .map(|bg| Self::parse_steps(&bg.steps))
            .unwrap_or_default();

        let mut scenarios: Vec<ParsedScenario> = feature
            .scenarios
            .iter()
            .map(Self::parse_scenario)
            .collect::<Result<Vec<_>>>()?;

        for rule in &feature.rules {
            let rule_tags: Vec<String> = rule.tags.iter().map(|t| t.to_string()).collect();
            for scenario in &rule.scenarios {
                let mut parsed = Self::parse_scenario(scenario)?;
                for tag in &rule_tags {
                    if !parsed.tags.contains(tag) {
                        parsed.tags.push(tag.clone());
                    }
                }
                scenarios.push(parsed);
            }
        }

        Ok(ParsedFeature {
            name: feature.name.clone(),
            description: feature.description.clone().filter(|d| !d.is_empty()),
            background_steps,
            scenarios,
        })
    }

    fn parse_scenario(scenario: &gherkin::Scenario) -> Result<ParsedScenario> {
        let steps = Self::parse_steps(&scenario.steps);
        let examples = Self::parse_examples(scenario)?;

        Ok(ParsedScenario {
            title: scenario.name.clone(),
            description: scenario.description.clone().filter(|d| !d.is_empty()),
            tags: scenario.tags.iter().map(|t| t.to_string()).collect(),
            steps,
            examples,
        })
    }

    fn parse_steps(steps: &[gherkin::Step]) -> Vec<ParsedStep> {
        steps
            .iter()
            .map(|step| {
                let doc_string = step.docstring.clone();
                let data_table = step.table.as_ref().map(Self::parse_data_table);

                ParsedStep {
                    keyword: step.keyword.trim().to_string(),
                    keyword_type: Self::step_type_to_string(&step.ty),
                    text: step.value.clone(),
                    doc_string,
                    data_table,
                }
            })
            .collect()
    }

    fn parse_data_table(table: &gherkin::Table) -> Vec<serde_json::Value> {
        let headers: Vec<&str> = table
            .rows
            .first()
            .map(|row| row.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();

        table
            .rows
            .iter()
            .skip(1)
            .map(|row| {
                let mut obj = serde_json::Map::new();
                for (idx, value) in row.iter().enumerate() {
                    if let Some(header) = headers.get(idx) {
                        obj.insert(header.to_string(), Self::parse_cell_value(value));
                    }
                }
                serde_json::Value::Object(obj)
            })
            .collect()
    }

    fn parse_cell_value(value: &str) -> serde_json::Value {
        if (value.starts_with('[') || value.starts_with('{') || value == "null")
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(value)
        {
            return json;
        }

        if let Ok(num) = value.parse::<i64>() {
            return serde_json::Value::Number(num.into());
        }

        if let Ok(num) = value.parse::<f64>()
            && let Some(n) = serde_json::Number::from_f64(num)
        {
            return serde_json::Value::Number(n);
        }

        match value.to_lowercase().as_str() {
            "true" => return serde_json::Value::Bool(true),
            "false" => return serde_json::Value::Bool(false),
            _ => {}
        }

        serde_json::Value::String(value.to_string())
    }

    fn parse_examples(scenario: &gherkin::Scenario) -> Result<Vec<ParsedExample>> {
        let mut parsed_examples = Vec::new();

        for examples_block in &scenario.examples {
            if let Some(table) = &examples_block.table {
                let headers: Vec<&str> = table
                    .rows
                    .first()
                    .map(|row| row.iter().map(|s| s.as_str()).collect())
                    .unwrap_or_default();

                let status_idx = headers.iter().position(|h| {
                    let lower = h.to_lowercase();
                    lower == "status"
                        || lower == "expected_status"
                        || lower == "expected_status_code"
                });

                for row in table.rows.iter().skip(1) {
                    let mut data = serde_json::Map::new();
                    let mut expected_status: Option<i16> = None;

                    for (idx, value) in row.iter().enumerate() {
                        if let Some(header) = headers.get(idx) {
                            if Some(idx) == status_idx {
                                expected_status = value.parse().ok();
                            } else {
                                data.insert(header.to_string(), Self::parse_cell_value(value));
                            }
                        }
                    }

                    parsed_examples.push(ParsedExample {
                        data: serde_json::Value::Object(data),
                        expected_status_code: expected_status,
                    });
                }
            }
        }

        Ok(parsed_examples)
    }

    fn step_type_to_string(step_type: &StepType) -> String {
        match step_type {
            StepType::Given => "Context".to_string(),
            StepType::When => "Action".to_string(),
            StepType::Then => "Outcome".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_feature() {
        let feature_code = r#"
Feature: Sign in
  User authentication system

  @signin
  Scenario Outline: User sign in
    Given I am an existing <provider> user
    And I have entered <email> in the form
    And I have entered <password> into the form
    When I press signin
    Then the result should be <status> on the screen

    Examples:
      | provider | email            | password   | status |
      | native   | test@example.com | Test123    | 200    |
      | native   | test@example.com | WrongPass  | 403    |
"#;

        let result = GherkinService::parse(feature_code);
        assert!(result.is_ok());

        let feature = result.unwrap();
        assert_eq!(feature.name, "Sign in");
        assert_eq!(feature.scenarios.len(), 1);

        let scenario = &feature.scenarios[0];
        assert_eq!(scenario.title, "User sign in");
        assert!(scenario.tags.contains(&"signin".to_string()));
        assert_eq!(scenario.steps.len(), 5);
        assert_eq!(scenario.examples.len(), 2);

        let first_example = &scenario.examples[0];
        assert_eq!(first_example.expected_status_code, Some(200));
        assert_eq!(
            first_example.data.get("email").and_then(|v| v.as_str()),
            Some("test@example.com")
        );
    }

    #[test]
    fn test_parse_background() {
        let feature_code = r#"
Feature: API with Background
  Background:
    Given the API server is running
    And I have a valid auth token

  Scenario: Get user profile
    When I GET /api/users/me
    Then status is 200
"#;

        let result = GherkinService::parse(feature_code);
        assert!(result.is_ok());

        let feature = result.unwrap();
        assert_eq!(feature.background_steps.len(), 2);
        assert_eq!(
            feature.background_steps[0].text,
            "the API server is running"
        );
        assert_eq!(
            feature.background_steps[1].text,
            "I have a valid auth token"
        );
        assert_eq!(feature.scenarios.len(), 1);
    }

    #[test]
    fn test_parse_doc_string() {
        let feature_code = r#"
Feature: API with Doc Strings
  Scenario: Create user
    When I POST /api/users with body:
      """
      {
        "email": "test@example.com",
        "name": "Test User"
      }
      """
    Then status is 201
"#;

        let result = GherkinService::parse(feature_code);
        assert!(result.is_ok());

        let feature = result.unwrap();
        let step = &feature.scenarios[0].steps[0];
        assert!(step.doc_string.is_some());

        let doc_str = step.doc_string.as_ref().unwrap();
        let json: serde_json::Value = serde_json::from_str(doc_str).unwrap();
        assert_eq!(
            json.get("email").and_then(|v| v.as_str()),
            Some("test@example.com")
        );
    }

    #[test]
    fn test_parse_data_table() {
        let feature_code = r#"
Feature: API with Data Tables
  Scenario: Create multiple users
    Given the following users exist:
      | email              | role  | active |
      | admin@test.com     | admin | true   |
      | user@test.com      | user  | false  |
    When I GET /api/users
    Then status is 200
"#;

        let result = GherkinService::parse(feature_code);
        assert!(result.is_ok());

        let feature = result.unwrap();
        let step = &feature.scenarios[0].steps[0];
        assert!(step.data_table.is_some());

        let table = step.data_table.as_ref().unwrap();
        assert_eq!(table.len(), 2);

        assert_eq!(
            table[0].get("email").and_then(|v| v.as_str()),
            Some("admin@test.com")
        );
        assert_eq!(table[0].get("role").and_then(|v| v.as_str()), Some("admin"));
        assert_eq!(table[0].get("active").and_then(|v| v.as_bool()), Some(true));

        assert_eq!(
            table[1].get("active").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn test_parse_cell_value_types() {
        assert_eq!(
            GherkinService::parse_cell_value("42"),
            serde_json::json!(42)
        );

        assert_eq!(
            GherkinService::parse_cell_value("3.25"),
            serde_json::json!(3.25)
        );

        assert_eq!(
            GherkinService::parse_cell_value("true"),
            serde_json::json!(true)
        );
        assert_eq!(
            GherkinService::parse_cell_value("FALSE"),
            serde_json::json!(false)
        );

        assert_eq!(
            GherkinService::parse_cell_value("null"),
            serde_json::json!(null)
        );

        assert_eq!(
            GherkinService::parse_cell_value("[1, 2, 3]"),
            serde_json::json!([1, 2, 3])
        );

        assert_eq!(
            GherkinService::parse_cell_value("hello"),
            serde_json::json!("hello")
        );
    }
}
