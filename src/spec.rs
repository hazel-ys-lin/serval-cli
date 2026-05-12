//! Permissive `.feature` file loader for real-world Gherkin exports.
//!
//! [`crate::gherkin::GherkinService::parse`] is strict — it expects
//! one `Feature:` per input and no `# language:` directive that
//! contradicts the keyword set. Real-world Gherkin files in the wild
//! commonly violate both:
//! - codegen tools emit multiple `Feature:` blocks into one file
//! - templates prepend `# language: <code>` directives even when
//!   keywords stay in English
//!
//! This module preprocesses input into chunks the strict parser can
//! swallow, then collects the per-Feature results into a `Vec`.
//!
//! Use [`crate::gherkin::GherkinService::parse`] directly when you
//! need strict mode (a single Feature, no preprocessing) — for
//! example when validating that authors wrote a clean `.feature`
//! file.
//!
//! ## Locale caveat
//!
//! [`parse_relaxed`] assumes English-keyword Gherkin. The
//! `# language:` strip means that a file written with non-English
//! keywords (e.g. real `功能:` / `情境:` zh-TW Gherkin) will fail to
//! parse after preprocessing. For that case, call
//! [`crate::gherkin::GherkinService::parse`] directly.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::frontmatter::{self, ApiFrontmatter, Frontmatter};
use crate::gherkin::{GherkinService, ParsedFeature};

/// Load a `.feature` file from disk and parse it permissively.
/// Returns one [`ParsedFeature`] per top-level `Feature:` block found
/// in the file. See [`parse_relaxed`] for the preprocessing rules.
pub fn load_file(path: &Path) -> Result<Vec<ParsedFeature>> {
    let content = std::fs::read_to_string(path)?;
    parse_relaxed(&content)
}

/// Parse a `.feature`-shaped string into one or more
/// [`ParsedFeature`]s.
///
/// Preprocessing applied before reaching `GherkinService::parse`:
/// - any `# language:` directive line is stripped
/// - the input is split on each line beginning with `Feature:`, so
///   files carrying multiple Features parse into multiple results
///
/// Chunks that contain no `Feature:` line (e.g. a leading comment
/// block before the first Feature) are skipped.
pub fn parse_relaxed(content: &str) -> Result<Vec<ParsedFeature>> {
    let preprocessed = strip_language_directives(content);
    split_features(&preprocessed)
        .into_iter()
        .filter(|c| c.contains("Feature:"))
        .map(|c| GherkinService::parse(&c))
        .collect()
}

fn strip_language_directives(s: &str) -> String {
    s.lines()
        .filter(|line| !line.trim_start().starts_with("# language:"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_features(s: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in s.lines() {
        if line.starts_with("Feature:") && !current.trim().is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// A `.feature` file on disk plus its parsed frontmatter and feature
/// blocks. Returned by [`discover`].
#[derive(Debug, Clone)]
pub struct SpecRecord {
    pub path: PathBuf,
    pub frontmatter: Option<Frontmatter>,
    pub features: Vec<ParsedFeature>,
}

impl SpecRecord {
    /// API metadata, if frontmatter provides one.
    pub fn api(&self) -> Option<&ApiFrontmatter> {
        self.frontmatter.as_ref().and_then(|f| f.api.as_ref())
    }

    /// Total scenario count across all features in the file.
    pub fn scenario_count(&self) -> usize {
        self.features.iter().map(|f| f.scenarios.len()).sum()
    }

    /// Unique tags across all scenarios (sorted, deduplicated).
    pub fn unique_tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self
            .features
            .iter()
            .flat_map(|f| f.scenarios.iter())
            .flat_map(|s| s.tags.iter())
            .map(String::as_str)
            .collect();
        tags.sort_unstable();
        tags.dedup();
        tags
    }
}

/// Walk `dir` recursively, load every `.feature` file, and return a
/// [`SpecRecord`] per file with its frontmatter and parsed features.
/// Returns an empty Vec when `dir` does not exist.
///
/// Fails fast on malformed frontmatter or Gherkin so the user gets a
/// pointer to the offending file rather than a silent skip.
pub fn discover(dir: &Path) -> Result<Vec<SpecRecord>> {
    let mut paths = Vec::new();
    if dir.exists() {
        walk_feature_files(dir, &mut paths)
            .map_err(|e| Error::System(format!("walking {}: {e}", dir.display())))?;
    }
    paths.sort();

    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| Error::System(format!("read {}: {e}", path.display())))?;
        let (frontmatter_block, body) = frontmatter::split(&raw)?;
        let features = parse_relaxed(body)?;
        out.push(SpecRecord {
            path,
            frontmatter: frontmatter_block,
            features,
        });
    }
    Ok(out)
}

fn walk_feature_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_feature_files(&path, out)?;
        } else if path.extension().and_then(|x| x.to_str()) == Some("feature") {
            out.push(path);
        }
    }
    Ok(())
}

/// Recursively list every `.feature` file under `dir`, sorted by
/// path. Missing or non-directory inputs yield an empty Vec. The
/// `spec validate` command consumes this to enumerate files without
/// also loading them.
pub fn collect_feature_paths(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if dir.exists() {
        let _ = walk_feature_files(dir, &mut out);
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_language_directive_before_parsing() {
        let input = "\
# language: zh-TW
Feature: hello
  Scenario: ok
    Given x
";
        let result = parse_relaxed(input).expect("must parse");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "hello");
    }

    #[test]
    fn splits_multi_feature_input_into_separate_results() {
        let input = "\
Feature: A
  Scenario: a1
    Given x

Feature: B
  Scenario: b1
    Given y
";
        let result = parse_relaxed(input).expect("must parse");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "A");
        assert_eq!(result[1].name, "B");
    }

    #[test]
    fn skips_leading_comment_chunk_before_first_feature() {
        let input = "\
# auto-generated by codegen — do not edit
# another comment line

Feature: X
  Scenario: x1
    Given x
";
        let result = parse_relaxed(input).expect("must parse");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "X");
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }

    #[test]
    fn discover_walks_subdirs_and_collects_records() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "a.feature",
            "Feature: A\n  Scenario: x\n    Given y\n",
        );
        write(
            tmp.path(),
            "sub/b.feature",
            "Feature: B\n  Scenario: x\n    Given y\n",
        );
        write(tmp.path(), "ignore.txt", "not a feature");

        let records = discover(tmp.path()).expect("discover");
        assert_eq!(records.len(), 2);
        // Sorted by path; a.feature before sub/b.feature
        assert!(records[0].path.ends_with("a.feature"));
        assert!(records[1].path.ends_with("sub/b.feature"));
    }

    #[test]
    fn discover_extracts_frontmatter_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "with_fm.feature",
            "\
---
api:
  path: /api/users
  method: POST
  collection: users
---
Feature: Users
  Scenario: x
    Given y
",
        );
        write(
            tmp.path(),
            "no_fm.feature",
            "Feature: Other\n  Scenario: x\n    Given y\n",
        );

        let records = discover(tmp.path()).expect("discover");
        let by_name = |stem: &str| {
            records
                .iter()
                .find(|r| r.path.file_stem().unwrap().to_string_lossy() == stem)
                .unwrap()
        };

        let with_fm = by_name("with_fm");
        let api = with_fm.api().expect("frontmatter has api block");
        assert_eq!(api.path, "/api/users");
        assert_eq!(api.method, "POST");
        assert_eq!(api.collection.as_deref(), Some("users"));

        let no_fm = by_name("no_fm");
        assert!(no_fm.api().is_none());
    }

    #[test]
    fn discover_returns_empty_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let records = discover(&missing).expect("missing dir should be empty, not error");
        assert!(records.is_empty());
    }

    #[test]
    fn unique_tags_dedup_and_sort() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "tagged.feature",
            "\
Feature: F
  @beta @alpha
  Scenario: one
    Given x
  @alpha
  Scenario: two
    Given x
",
        );
        let records = discover(tmp.path()).expect("discover");
        let tags = records[0].unique_tags();
        assert_eq!(tags, vec!["alpha", "beta"]);
    }
}
