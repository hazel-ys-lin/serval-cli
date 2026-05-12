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
