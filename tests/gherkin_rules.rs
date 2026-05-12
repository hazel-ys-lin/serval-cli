//! Integration tests for Gherkin `Rule:` block handling.
//!
//! Standard Gherkin (Gherkin 6+) lets a Feature contain multiple
//! `Rule:` blocks, each holding its own scenarios. The Rust `gherkin`
//! crate exposes them as `Feature.rules[].scenarios` separate from the
//! top-level `Feature.scenarios`; missing that walk silently drops
//! every scenario inside a Rule (regression v2 inherited).
//!
//! Tags written on the Rule line apply to all scenarios inside, per
//! the Cucumber tag-inheritance contract; without propagation,
//! `serval run --tag @foo` would miss scenarios whose tag sat on the
//! Rule rather than the Scenario itself.

use serval_cli::gherkin::GherkinService;
use std::fs;

fn load() -> serval_cli::gherkin::ParsedFeature {
    let content =
        fs::read_to_string("tests/fixtures/rules_basic.feature").expect("fixture must exist");
    GherkinService::parse(&content).expect("fixture must parse")
}

#[test]
fn scenarios_inside_rules_are_collected() {
    let feature = load();
    assert_eq!(
        feature.scenarios.len(),
        3,
        "expected 3 scenarios (2 in Login rule, 1 in Logout rule)"
    );

    let titles: Vec<&str> = feature.scenarios.iter().map(|s| s.title.as_str()).collect();
    assert!(titles.contains(&"User logs in with valid credentials"));
    assert!(titles.contains(&"Rate limited after repeated failures"));
    assert!(titles.contains(&"User logs out"));
}

#[test]
fn rule_tags_propagate_onto_inner_scenarios() {
    let feature = load();

    let by_title = |t: &str| {
        feature
            .scenarios
            .iter()
            .find(|s| s.title == t)
            .unwrap_or_else(|| panic!("scenario {t:?} missing"))
    };

    let login_ok = by_title("User logs in with valid credentials");
    assert!(
        login_ok.tags.iter().any(|t| t == "authentication"),
        "Rule's @authentication tag must propagate; got tags = {:?}",
        login_ok.tags
    );

    let rate_limited = by_title("Rate limited after repeated failures");
    assert!(rate_limited.tags.iter().any(|t| t == "authentication"));
    assert!(
        rate_limited.tags.iter().any(|t| t == "rate-limit"),
        "Scenario's own @rate-limit tag must also be present; got tags = {:?}",
        rate_limited.tags
    );

    let logout = by_title("User logs out");
    assert!(
        logout.tags.iter().any(|t| t == "management"),
        "different Rule's @management tag must propagate; got tags = {:?}",
        logout.tags
    );
    assert!(
        !logout.tags.iter().any(|t| t == "authentication"),
        "tag from a different Rule must NOT leak; got tags = {:?}",
        logout.tags
    );
}
