//! Integration test: `spec::load_file` consumes a multi-Feature
//! `.feature` file whose `# language:` directive contradicts its
//! English keyword set, then collects per-Feature results.
//!
//! The fixture is synthetic — it mirrors only the *structure* of
//! codegen-emitted Gherkin (multi-Feature, language-directive
//! mismatch, Rule-grouped scenarios, duplicated and own scenario
//! tags). It is not anyone's real spec; real `.feature` files live
//! in user repos under `specs/`, not in this crate.

use serval_cli::spec;
use std::path::Path;

const FIXTURE: &str = "tests/fixtures/codegen_export.feature";

fn load() -> Vec<serval_cli::gherkin::ParsedFeature> {
    spec::load_file(Path::new(FIXTURE)).expect("fixture must parse")
}

#[test]
fn multi_feature_input_yields_one_result_per_feature() {
    let features = load();
    assert_eq!(features.len(), 3, "expected 3 top-level Features");

    let names: Vec<&str> = features.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Sign-up", "Login", "Logout"]);
}

#[test]
fn scenario_counts_match_per_feature() {
    let features = load();
    let by_name = |n: &str| {
        features
            .iter()
            .find(|f| f.name == n)
            .unwrap_or_else(|| panic!("feature {n:?} missing"))
    };

    assert_eq!(by_name("Sign-up").scenarios.len(), 3);
    assert_eq!(by_name("Login").scenarios.len(), 1);
    assert_eq!(by_name("Logout").scenarios.len(), 1);
}

#[test]
fn rule_tags_propagate_dedup_and_do_not_leak_across_rules() {
    let features = load();
    let signup = features
        .iter()
        .find(|f| f.name == "Sign-up")
        .expect("Sign-up missing");

    let by_title = |t: &str| {
        signup
            .scenarios
            .iter()
            .find(|s| s.title == t)
            .unwrap_or_else(|| panic!("scenario {t:?} missing"))
    };

    let new_email = by_title("New email registers");
    let happy_count = new_email.tags.iter().filter(|t| *t == "happy-path").count();
    assert_eq!(
        happy_count, 1,
        "duplicated `@happy-path` on the Rule must dedup; got tags = {:?}",
        new_email.tags
    );

    let rate_limited = by_title("Rate limited after repeated attempts");
    assert!(rate_limited.tags.iter().any(|t| t == "happy-path"));
    assert!(
        rate_limited.tags.iter().any(|t| t == "rate-limit"),
        "scenario's own tag must merge with Rule tags; got tags = {:?}",
        rate_limited.tags
    );

    let duplicate = by_title("Duplicate email blocks sign-up");
    assert!(duplicate.tags.iter().any(|t| t == "validation"));
    assert!(
        !duplicate.tags.iter().any(|t| t == "happy-path"),
        "tag from a sibling Rule must NOT leak; got tags = {:?}",
        duplicate.tags
    );
}
