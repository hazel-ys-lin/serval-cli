# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).
Pre-1.0 the API surface is unstable — breaking changes can happen in
any minor or pre-release bump.

## [Unreleased]

### Added

- CLI-focused `Cargo.toml` dependency set: clap 4 (derive + env),
  reqwest (blocking + json), tokio, serde / serde_json / toml,
  gherkin, tracing / tracing-subscriber, thiserror, uuid, time,
  dotenvy, async-trait. Versions mirror serval-run-v2 v0.2.0 where
  they overlap so the upcoming Gherkin/runner port doesn't fight
  version drift.
- Package metadata in `Cargo.toml`: description, repository, license,
  readme, keywords, categories.
- `README.md` framing `serval-cli` as a CLI tool (not a web service),
  with the exit-code contract that CI and agent eval loops will
  branch on, and an Origin section explaining the pivot from
  `serval-run-v2` v0.2.0.
- CLI scaffold ported from `serval-run-v2` v0.2.0: `src/lib.rs`,
  `src/cli/{mod,exit,output}.rs`, `src/cli/commands/{mod,status}.rs`,
  `src/bin/serval.rs`. The `status` subcommand hits a placeholder
  `/health` endpoint (no upstream server yet; Phase 3 mock will
  define one).
- `HealthResponse` shape kept minimal at `status` + `version`;
  v2-specific postgres / mongodb / redis fields intentionally
  dropped.

### Changed

- CLI binary renamed `servalrun` → `serval`. Aligns the command name
  with the `serval-cli` repo / package and leaves room for non-`run`
  subcommands (`serval mock`, `serval lint`, ...). `CLAUDE.md` and
  `README.md` updated to match.

### Removed

- `src/main.rs` hello-world stub. The `serval` binary at
  `src/bin/serval.rs` is now the sole entry point.

### Added (core libraries)

- `src/error.rs`: lib-crate `Error` enum (`Spec`, `System`, `Io`,
  `Http`) and `Result<T>` alias. `Spec` corresponds to CLI exit
  code 3, the others to exit code 2. Test-assertion failures stay
  outside this hierarchy and surface as `pass: false` on a
  `TestResult`.
- `src/gherkin.rs`: Gherkin feature-file parser ported from
  `serval-run-v2` v0.2.0 `services/gherkin.rs` (verbatim except
  for `AppError::Validation` → `Error::Spec`). Exposes
  `GherkinService::parse` producing `ParsedFeature` /
  `ParsedScenario` / `ParsedStep` / `ParsedExample` DTOs. Inline
  tests for feature / background / doc-string / data-table /
  cell-type inference ported as-is.
- `src/runner.rs`: async HTTP test runner ported from
  `serval-run-v2` v0.2.0 `services/test_runner.rs` and decoupled
  from v2's entity layer. New plain DTOs `ApiSpec` (`endpoint` +
  `http_method`) and `EnvSpec` (`base_url`, renamed from v2's
  `domain_name`) replace the `Api` / `Environment` SeaORM
  entities. `TestRunner::run_scenario` now consumes a
  `&ParsedScenario` directly — no JSON round-trip through DB
  columns. Inline tests for placeholder substitution / status
  extraction / JSON containment ported verbatim.

### Changed (core libraries)

- `TestResult` shape: drops `scenario_id` / `api_id` (`Uuid` —
  no DB to anchor IDs to); adds `scenario_title: String` for
  identification in the eventual `.serval/reports/<ts>.json`
  output. `api` / `environment` references retire alongside.
- `ParsedExample.expected_status_code` is `Option<i16>`
  (Examples-driven, may be absent). v2's
  `TestExample.expected_response_body` is intentionally **not**
  ported — expected-body assertions now come strictly from `Then`
  steps, not from a magic Examples column. Cleaner separation
  between row data and response expectations.

### Fixed

- `GherkinService::parse` now walks `Feature.rules[].scenarios` in
  addition to `Feature.scenarios`. Scenarios written inside a
  Gherkin 6+ `Rule:` block were previously dropped from
  `ParsedFeature.scenarios`, so a perfectly valid Rule-organized
  spec parsed cleanly but reported zero scenarios — a regression
  inherited from `serval-run-v2`'s parser that only surfaced once a
  Rule-style spec was thrown at it.
- Rule-level tags now propagate onto each inner scenario
  (deduplicated against the scenario's own tags). Matches the
  Cucumber tag-inheritance contract so `serval run --tag @foo`
  filters correctly when `@foo` sits on the `Rule:` line rather
  than directly on a `Scenario:`.
- `TestRunner::run_scenario` runs concrete scenarios (`Scenario:`
  blocks with no `Examples:` table) exactly once with
  `Value::Null` as the implicit row data. Previously the empty
  examples iterator meant the scenario was silently never
  executed — fine for `Scenario Outline:` flows but broken for
  one-off concrete scenarios common in event-sourcing /
  specification-by-example style specs.
- `TestResult.request_time` serializes as an RFC 3339 string
  (e.g. `"2026-05-12T03:30:12.923819Z"`) instead of the
  9-element integer array `time::OffsetDateTime` defaults to.
  Makes `serval run --json` output and the upcoming JSON report
  files (PR-1B) consumable by `jq` and agent loops without
  custom decoding. Requires the `serde-well-known` + `parsing`
  features on the `time` crate.

### Added (CLI)

- `serval run <path>` subcommand: first end-to-end execution
  path. Reads a `.feature` file (optionally with YAML
  frontmatter), parses it via `spec::parse_relaxed`, and executes
  every scenario against an HTTP target with `TestRunner`. Flags:
  `--base-url URL` (or `$SERVAL_BASE_URL`, required); `--endpoint`
  / `--method` (required unless frontmatter provides `api.path` /
  `api.method`); `--timeout SECS` (default 30); `--json`. Exit
  codes follow the public contract — `0` all pass, `1` any
  assertion fails, `2` system / network error, `3` bad input.
  Output is per-scenario `[PASS] / [FAIL]` lines plus a summary
  by default, or pretty-printed JSON with `--json`.
- `serval run` writes a JSON report to
  `<cwd>/.serval/reports/<rfc3339-timestamp>.json` after every
  run (filename colons replaced with dashes for Windows fs
  safety). Report schema v1 captures `schema_version`,
  `started_at` / `finished_at`, `source_file`, the resolved
  `target` (base_url + endpoint + method), a `summary` block
  (total / passed / failed), and the full `results` array.
  Override the directory with `--report-dir <path>` or
  `$SERVAL_REPORT_DIR`; skip writing with `--no-report`. In
  table mode the report path is appended to stdout; with
  `--json`, it goes to stderr so stdout stays pure JSON. New
  module `src/report.rs`. New dev-dep `tempfile = "3"` for
  isolated integration tests.
- `serval history` subcommand: list past run reports under
  `.serval/reports/` (or `--report-dir <path>`) sorted by
  `started_at` descending. `--limit N` caps output (default 20);
  `--json` emits a JSON array with `id` / `started_at` /
  `finished_at` / `source_file` / `target` / `summary` per
  entry. Empty directory prints `no reports found`.
- `serval diff <before> <after>` subcommand: compare two run
  reports. IDs accept exact filename (without `.json`), unique
  prefix, or the keywords `latest` / `previous`. Surfaces
  scenario flips (PASS↔FAIL with status code delta), added /
  removed scenarios, plus `source_file` / `target` change
  warnings and per-side summary counts. `--json` emits a tagged
  `ScenarioChange` enum (`flipped` / `added` / `removed`)
  alongside `before_id`, `after_id`, `source_changed`,
  `target_changed`, `summary_before`, `summary_after`. Exits 3
  on ambiguous prefixes or missing IDs.
- `src/report.rs` gains `ReportRecord`, `read(path)`,
  `list(dir)`, and `resolve(dir, id)` for the new subcommands to
  consume (inline tests cover sort order, prefix matching,
  ambiguous-prefix errors, and the `latest` / `previous`
  keywords).
- `src/frontmatter.rs`: optional YAML frontmatter parser for
  `.feature` files. `Frontmatter { api, implements }` with
  `ApiFrontmatter { path, method, collection }`. `split(content)`
  returns `(None, content)` when no `---` opener is present, so
  files without frontmatter pass through transparently.
- New deps: `serde_yml = "0.0.12"` (regular, for frontmatter);
  `assert_cmd = "2"` + `predicates = "3"` (dev, for integration
  tests that spawn the `serval` binary).

### Added (spec loader)

- `src/spec.rs`: permissive `.feature` file loader. Two entry
  points: `spec::load_file(path)` (read + parse) and
  `spec::parse_relaxed(content)` (parse only). Preprocessing
  strips `# language:` directives and splits multi-Feature input
  on each top-level `Feature:` line, then forwards each chunk to
  the strict `GherkinService::parse`. Returns `Vec<ParsedFeature>`
  rather than a single feature.
- `tests/fixtures/codegen_export.feature` +
  `tests/spec_loader.rs`: synthetic regression test verifying the
  preprocessor handles multi-Feature files plus a `# language:` /
  keyword-set mismatch in one shot, and that Rule-level tag
  propagation (from the Fixed section above) survives the chunk
  split. Fixture is structural-only — real `.feature` specs live
  in user repos under `specs/`, not in this crate.

### Excluded by design

- Web-app and multi-user deps inherited from `serval-run-v2` are
  intentionally absent: axum, tower / tower-http / tower_governor,
  utoipa / utoipa-swagger-ui, sea-orm, sqlx, mongodb, bson, redis,
  jsonwebtoken, argon2, rust_decimal.

## [0.1.0-alpha.0] - 2026-05-11

Baseline anchor for the CLI successor to
[`serval-run-v2`](https://github.com/hazel-ys-lin/serval-run-v2)
(frozen at `v0.2.0`). See the README's Origin section for the pivot
rationale.

### Added

- Initial cargo skeleton (`src/main.rs` hello-world stub).
- `CLAUDE.md` development guidelines (project identity, critical
  rules, CLI conventions, exit codes, commit/PR norms).
- `LICENSE` (MIT).
- `.gitignore`.

[Unreleased]: https://github.com/hazel-ys-lin/serval-cli/compare/v0.1.0-alpha.0...HEAD
[0.1.0-alpha.0]: https://github.com/hazel-ys-lin/serval-cli/releases/tag/v0.1.0-alpha.0
