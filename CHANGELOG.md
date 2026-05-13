# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).
Pre-1.0 the API surface is unstable — breaking changes can happen in
any minor or pre-release bump.

## [Unreleased]

## [0.5.0] - 2026-05-13

Phase 3 complete: the step-pattern engine grows the seven knobs
needed to translate codegen-style event-sourcing Gherkin into a
real REST backend's wire format without rewriting either side.
The release groups Phase 3.0 (scenario variables +
`capture_response` + per-pattern headers + `--header` CLI flag),
3.1 (`DocStringTemplate { rename, defaults }` body reshape),
3.2 (drop vacuous `{}` doc-string assertion — strict mode now
catches what was silently false-PASSing), 3.3
(`DocStringTemplate.overrides` — forced values win over the doc-
string), 3.4 (`AssertBodyMatchesAt { pointer }` — scoped deep-
match against a JSON sub-document), 3.5
(`HttpRequest.accepted_status` — seed POST idempotency knob),
3.6 (stream-id symbol table — templated `capture_response` keys
plus two-pass `substitute_template`), and 3.7 (`doc_captures` +
override-value templating + multi-pass variable substitution).

A real-world dogfood against the jsjh-backend-v2 REST API
exercised every engine path end-to-end. See CHANGELOG.md /
`examples/event-sourcing.toml` for the full surface; the
`patterns.toml` schema is backward compatible — every new field
is optional and absent values restore Phase 2.x behaviour.

### Added

- **Phase 3.0** — scenario variables and per-pattern headers,
  the engine groundwork for real-backend dogfood.
  - `Action::HttpRequest` grows a `capture_response: HashMap<String, String>`
    field. Each entry is `var_name → JSON pointer` (RFC 6901);
    after the response body lands, the pointer is evaluated and
    the result is stored on `ScenarioContext.variables` under
    `var_name`.
  - `Action::HttpRequest` grows a `headers: HashMap<String, String>`
    field for per-pattern headers. Values support template
    substitution (regex captures + scenario variables).
  - New `ValueSource::Variable(name)` variant lets `body_from`
    pull a previously captured value as the whole request body.
  - Template substitution learns the `{{$name}}` syntax for
    scenario variables (alongside the existing `{{name}}` regex
    capture syntax). Unknown variables expand to empty, matching
    the existing capture-group behaviour.
  - New `--header "Key: Value"` CLI flag (repeatable). Parsed
    into `TestConfig.custom_headers` and attached to every HTTP
    firing — both pattern-driven `Action::HttpRequest` calls and
    the frontmatter fallback.
- **Phase 3.1** — `ValueSource::DocStringTemplate { rename,
  defaults }` reshapes a step's doc-string before using it as a
  request body. Targets codegen Gherkin whose body shape
  disagrees with the real backend (Gherkin `{"username": …}`
  → v2's `{"account": …}` via `rename`; required fields absent
  from the Gherkin shape filled from `defaults`).
  - Resolution: read doc-string → parse as JSON object → apply
    `rename` (old → new key) → merge `defaults` underneath; doc-
    string keys win on collision. Returns `None` if the step has
    no doc-string, the doc-string is not valid JSON, or it is
    not a top-level object.
  - TOML schema:
    `body_from = { kind = "doc_string_template", rename = { … },
    defaults = { … } }`. Both fields are optional (default to
    empty map / JSON null respectively).
- **Phase 3.3** — `DocStringTemplate` grows an `overrides` field
  that wins OVER the doc-string. Resolution now stacks three
  layers: `defaults` (bottom) ← renamed doc-string ← `overrides`
  (top). Targets cases where a Gherkin literal collides with a
  backend validator pattern-wide (the dogfood case: Gherkin's
  `"password": "pass1234"` is 8 chars, v2's `Password` validator
  requires `> 8` → `overrides = { password = "Pass12345!" }`
  rewrites every Login / CreateAccount body without touching
  the Gherkin).
  - Optional field on the existing `doc_string_template` body
    source; defaults to JSON null (no overrides).
  - Non-object `overrides` is silently treated as empty,
    matching the existing tolerance for non-object `defaults`.
- **Phase 3.4** — `Action::AssertBodyMatchesAt { pointer }` plus
  `ScenarioContext.expected_body_pointer`. Scopes the doc-string
  deep-partial match to a JSON sub-document (RFC 6901 pointer)
  of the response body, closing the wire-shape gap when codegen
  Gherkin asserts a bare collection (`Then the view returns:
  [...]`) but the backend wraps it (`{users: [...]}` from
  v2's `GET /users/list`).
  - TOML: `[[pattern.actions]] type = "assert_body_matches_at"
    pointer = "/users"`.
  - Overwrites whatever the built-in `AssertBodyMatches` set on
    the same step — user patterns fire after built-ins, so the
    scoped form always wins when both apply.
  - Same vacuous-`{}` skip as `AssertBodyMatches` (Phase 3.2).
  - Validator surfaces a clear `Pointer /xyz resolved to nothing`
    failure when the pointer doesn't exist in the response.
- **Phase 3.5** — `Action::HttpRequest` grows an optional
  `accepted_status: Vec<i16>` field. When non-empty, the
  response status code must appear in the list — otherwise the
  step aborts with a clear `HTTP <method> <url> returned status
  <n>, not in accepted_status [...]` error. Empty / absent
  preserves the existing "accept any status" behaviour.
  - Use case: cross-scenario seed POST idempotency. A pattern
    like `Given the AccountCreated event ...` firing `POST
    /users/create` against a stateful backend gets 201 on a
    fresh DB and 409 once the user pre-exists. Declaring
    `accepted_status = [201, 409]` treats either as success
    and lets the scenario continue without taking a dependency
    on DB reset between runs.
  - The unsuccessful response is still recorded on
    `ScenarioContext.responses` so post-mortem reports show
    what the backend returned.
- **Phase 3.6** — stream-id symbol table via templated
  `capture_response` keys + two-pass `substitute_template`.
  Closes the stream-id ↔ backend-UUID gap that blocked update /
  delete patterns from referencing seeded entities by their
  Gherkin stream id.
  - `capture_response` keys are now template-substituted at
    capture time. A seed pattern can declare
    `capture_response = { "user_for_{{stream}}" = "/id" }` and
    the captured UUID lands under a key derived from the step's
    `stream` regex capture (e.g. `user_for_acc-001`).
  - `substitute_template` runs two passes — regex captures
    (`{{name}}`) first, scenario variables (`{{$name}}`) second
    — so a later step can nest a capture inside a variable name
    (`endpoint_template = "/users/delete/{{$user_for_{{stream}}}}"`).
  - Allowed characters inside template names expanded from
    `\w+` to `[\w.\-:]+` so stream ids like `acc-001` round-trip
    through the variable key.
- **Phase 3.7** — body-field templating from doc-string contents.
  Three engine knobs land together to close the case where a
  Gherkin step's doc-string carries a stream-id buried in a body
  field (Gherkin's `"teamId": "team-001"`) that the backend
  expects as a captured UUID:
  - `Action::HttpRequest` grows `doc_captures: HashMap<String,
    String>` (TOML: `doc_captures = { team_stream = "/teamId" }`).
    Before the request fires, the doc-string is parsed as JSON
    and each pointer is evaluated; the result lands in scenario
    variables, available to template substitutions for endpoint
    / headers / body.
  - String values inside `DocStringTemplate.defaults` /
    `overrides` are run through `substitute_template` before the
    merge — so an override like
    `team_id = "{{$team_for_{{$team_stream}}}}"` resolves
    against captured variables to land a UUID in the body.
    `substitute_value_strings` walks nested objects + arrays,
    leaving non-string leaves untouched.
  - `substitute_template`'s variable-pass loops until stable
    (bounded at 5 iterations), so a variable's name can contain
    another variable reference. Resolves
    `{{$team_for_{{$team_stream}}}}` to the UUID in two
    iterations.

### Changed

- **Phase 3.2** — `Action::AssertBodyMatches` now skips an empty
  `{}` doc-string instead of setting `expected_body = {}`. Codegen
  Gherkin commonly writes `Then ... emitted with: {}` as
  documentation of "an event of this shape is emitted" without
  asserting any field; the prior deep-partial match passed any
  response body trivially, producing silent false-PASS reports.
  Strict mode now flags the missing assertion. Empty arrays are
  intentionally NOT flagged — `[]` plausibly means "expect empty
  list"; tightening that requires an explicit assert-equals
  action, not a vacuous-PASS silencer.
  - Behaviour change visible in `tests/dogfood_event_sourcing`:
    the fixture's `Anonymous logs in` scenario was vacuously
    passing because its `Then ... emitted with: {}` body
    assertion matched anything; it now correctly surfaces as
    FAIL with the strict-mode hint.
- `patterns::apply` swapped its `&Client, &str` parameter pair
  for an `&ApplyContext` struct bundling client, base URL, and
  global headers. Pure refactor — runner integration unchanged.
- `ScenarioContext` grew a `variables: HashMap<String, Value>`
  field, populated by `capture_response` and read by template
  substitution / `ValueSource::Variable`.

## [0.4.0] - 2026-05-12

Phase 2 complete: the step-pattern engine ships with a
two-tier (built-in + user TOML) table, multi-step state machine
driven by `Action::HttpRequest`, doc-string body + assertion,
failure-mode `operation fails with` step support, and
strict-mode vacuous-PASS detection. A 53-scenario
event-sourcing codegen Gherkin export was used to validate the
engine end-to-end against a mock event-store backend; vacuous
passes that masked pattern-coverage gaps in Phase 2.5 are now
zero.

The step-pattern engine is the headline new capability: the CLI
now translates Gherkin step text into HTTP requests + assertions
via a regex-driven dispatch table. Built-in patterns cover
generic HTTP-shape Gherkin (status / body-contains / header /
query param / request-body) and the new doc-string assertions.
Users layer project-specific patterns on top via
`patterns.toml`, including HTTP-firing patterns that turn a
scenario into a multi-step state machine. See
`examples/event-sourcing.toml` for a working set against the
event-sourcing convention.

### Added

- **Phase 2.6 P0** — failure-mode step support. Two new actions:
  `AssertExpectedStatusInRange { min, max }` (sets
  `expected_status` to a closed range matcher) and
  `AssertBodyContainsFromMatchGroup { group }` (pushes a named
  regex capture into `expected_body_contains`).
  `examples/event-sourcing.toml` now ships a user pattern for
  `Then the operation fails with: <msg>` that fires both —
  asserting 4xx + body contains the message.
- `runner::StatusMatcher` enum (`Exact(i16) | Range { min, max }`)
  replaces the prior plain `i16` for `expected_status`.
- **Phase 2.6 P1** — strict-mode vacuous-PASS detection. By
  default a scenario that runs to end without setting any
  assertion (`expected_status` / `expected_body` /
  `expected_body_contains`) now FAILs with a clear message
  pointing at the new `--allow-no-assertions` flag. This guards
  against the silent false-green pattern-coverage gap surfaced
  by the Phase 2.5 dogfood (23 / 53 scenarios were vacuously
  passing).
- Multi-action patterns: a single `[[pattern]]` can declare
  multiple `[[pattern.actions]]` entries. All actions fire in
  order when the regex matches.

### Changed

- **Breaking (pre-1.0)** patterns.toml schema: `[pattern.action]`
  inline table → `[[pattern.actions]]` table array (or
  `actions = [{...}, ...]`). One-pattern-one-action specs
  migrate by renaming the section header. The TOML loader
  rejects empty `actions = []` arrays.
- `ScenarioContext::expected_status` is now
  `Option<StatusMatcher>` rather than `Option<i16>`. Callers
  building a context from an Examples-table row use
  `StatusMatcher::Exact(n)` (handled by the runner).
- `TestConfig` grew `allow_no_assertions: bool` (default
  `false`).

- **Phase 2.5** — first end-to-end dogfood of the step-pattern
  engine. `examples/event-sourcing.toml` ships a happy-path
  user-pattern set that lets event-sourcing-style codegen Gherkin
  (using the
  `Given the <Event> event has occurred on stream "<id>":` /
  `When <Actor> sends <Command> on stream "<id>":` /
  `When the <View> view is queried` /
  `Then the <Event> event is emitted with:` /
  `Then the view returns:` convention) run verbatim against a
  backend exposing `POST /streams/{id}/events/{Event}`,
  `POST /streams/{id}/commands/{Cmd}`, and `GET /views/{View}`.
- Integration test `tests/dogfood_event_sourcing.rs` runs three
  scenarios (a list-view deep-match, a command with emitted-event
  assertion, and an empty-event assertion) against an in-process
  httpmock backend using the example patterns unchanged.
- **Phase 2.4** — `Then` step doc-string drives `expected_body`.
  New built-in pattern `AssertBodyMatches` parses a triple-quoted
  block on a `Then` step as JSON and runs a deep partial match
  against the response body (via the existing `json_contains`).
  New built-in patterns `SetRequestBodyFromDocString` (Given /
  When) replace the pre-2.4 unconditional doc-string-to-request-body
  parse — same semantics for Given / When, fixed semantics for
  Then.
- TOML schema: `type = "set_request_body_from_doc_string"` and
  `type = "assert_body_matches"` for user patterns.
- `patterns::substitute_placeholders` is now a `pub fn` reused
  by both the runner and pattern actions.
- **Phase 2.3** — multi-step state machine in the runner. A
  scenario can now fire multiple HTTP requests via `Action::HttpRequest`
  patterns, each recorded as an `HttpResponse` entry on the
  `ScenarioContext`. Validation runs against the *last* response.
- New `ValueSource` enum (`MatchGroup` / `DocString` / `Literal`)
  for data-driven action arguments. `Action::HttpRequest::body_from`
  consumes it today; future variants can reuse it.
- User `patterns.toml` now accepts `type = "http_request"` with
  `method`, `endpoint_template` (supports `{{group_name}}`
  substitution from named regex captures), and an optional
  `body_from = { kind = "match_group" | "doc_string" | "literal", … }`.

### Changed

- `runner::process_step` no longer parses doc strings
  unconditionally into `request_body`. The behaviour is now
  driven by built-in patterns (see Phase 2.4 above). This is a
  fix: previously a `Then ... <doc string>` would silently
  pollute `request_body`, which then leaked into the implicit
  end-of-scenario request in single-step specs.
- `runner::StepContext` renamed to `ScenarioContext` (the state
  is scenario-wide, not per-step) and grew a
  `responses: Vec<HttpResponse>` field.
- `patterns::apply` and `runner::process_step` are now `async`
  and return `Result<()>` — `Action::HttpRequest` can fail with
  a transport error mid-scenario, which surfaces as a failed
  `TestResult` for that example.
- `runner::execute_request` now returns `HttpResponse` instead
  of `(status, body)`.
- Specs without any `Action::HttpRequest` pattern fall back to
  the pre-2.3 single-request behaviour driven by the frontmatter
  `api.path` / `api.method` (or `--endpoint` / `--method`), so
  existing `.feature` files keep working unchanged.

### Removed

- `TestRunner::substitute_placeholders` method. Use
  `patterns::substitute_placeholders` directly. Breaking only
  for external callers; the only in-tree callsites were inside
  `runner.rs` itself.

## [0.3.1] - 2026-05-12

Privacy + packaging follow-up to v0.3.0. No `serval` CLI
behavioural change; first release to ship prebuilt binaries.

### Added

- Prebuilt binary distribution via cargo-dist 0.31.0. Tag pushes
  trigger `.github/workflows/release.yml`, which cross-builds for
  four targets (macOS arm64 + x86_64, Linux x86_64 + arm64),
  packages `.tar.xz` archives with the `serval` binary + LICENSE
  + README + CHANGELOG, and uploads a
  `serval-cli-installer.sh` shell installer alongside.
- README `## Install` section covering the `curl | sh`
  installer, `cargo install --git`, and manual `.tar.xz`
  download.

### Changed

- `CLAUDE.md` scrubbed of the `Author email` bullet and the
  entire `User preferences` section. Those were per-author
  behavioural context that already lived in local memory and did
  not belong in a public project guideline doc. The `OSENSE`
  employer mention is gone.

## [0.3.0] - 2026-05-12

Phase 0 (lite-native CLI scaffold) and Phase 1 (CLI primary
interface) complete. Version line continues from `serval-run-v2`
v0.2.0 — the binary is now `serval` and lives in this CLI-focused
repo. Phase 2 (specs as git source of truth, manifest lockfile) is
next per the roadmap.

The full subcommand surface as of v0.3.0:

```text
serval status                                 (placeholder)
serval run <path> [--env NAME | --base-url URL] [--endpoint P]
                  [--method M] [--report-dir DIR] [--no-report]
                  [--json]
serval history [--limit N] [--report-dir DIR] [--json]
serval diff <before-id> <after-id> [--report-dir DIR] [--json]
serval api {list, show <pattern>, find <query>} [--dir DIR] [--json]
serval env {list, show NAME, set NAME --base-url URL
            [--make-default], remove NAME} [--config-file PATH]
            [--json]
serval config {path, show} [--config-file PATH] [--json]
serval spec validate [<path>] [--json]
```

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
- `serval api {list, show <pattern>, find <query>}` subcommands:
  inspect `.feature` specs on disk. Default scan directory is
  `specs/` in the cwd; override with `--dir <path>`.
  - `list` shows every spec carrying YAML frontmatter with
    `api.{path,method}`, plus a scanned-count footer. Specs
    without frontmatter are intentionally omitted (known
    follow-up tracked in memory).
  - `show <pattern>` renders one spec's detail (api block,
    `implements`, features, scenarios). Pattern is a
    case-insensitive substring matched against `api.path`,
    `api.method`, `api.collection`, file path, feature name, or
    scenario tag. Multiple matches list candidates and exit 3;
    no match exits 3.
  - `find <query>` lists all matching API specs using the same
    substring rules — `find post`, `find @happy-path`,
    `find users` all work.
  - `--json` emits machine-readable shapes for each subcommand
    (`list` / `find` → array; `show` → object with nested
    `features`).
- `src/spec.rs` gains `SpecRecord { path, frontmatter, features }`
  + helper methods (`api()`, `scenario_count()`, `unique_tags()`)
  + `discover(dir)` that walks `.feature` files recursively and
  loads each. Five new inline tests cover sub-directory walking,
  frontmatter extraction, missing-dir handling, and tag dedup.
- `serval env {list, show, set, remove}` subcommands manage named
  environments in `~/.serval/config.toml` (override with
  `--config-file <path>` or `$SERVAL_CONFIG_FILE`). Each env
  carries a `base_url` (auth tokens / headers deferred). `set`
  accepts `--make-default` to wire the env to the config's
  `default_env` field; `remove` clears `default_env` when it
  pointed at the deleted entry. `list` table marks the default
  env; `--json` flag emits `[{ name, base_url, is_default }]`.
- `serval config {path, show}` prints the resolved config file
  path (handy for `vim "$(serval config path)"`) or the loaded
  contents (TOML by default, JSON with `--json`).
- `serval run` gains `--env <name>` and `--config-file <path>`.
  Base-URL resolution chain: explicit `--base-url` flag wins; if
  absent, `--env <name>` is looked up in the config; if no `--env`,
  the config's `default_env` is consulted. Missing all three is
  an exit-3 error with an actionable message
  (`serval env set NAME --base-url URL --make-default`).
- New module `src/config.rs` exposing `Config { default_env, envs
  }`, `EnvConfig { base_url }`, `default_path()` (uses
  `$SERVAL_CONFIG_FILE` then `$HOME/.serval/config.toml`),
  `load(path)` (missing file = empty Config), `save(path, &cfg)`,
  and `Config::resolve_env(name_or_default)`. Five inline tests
  cover empty roundtrip, missing-file default, save/load
  roundtrip, default_env fallback, and unknown-env handling.
- `serval spec validate [<path>]` subcommand: parse-checks
  `.feature` files (frontmatter + Gherkin) without firing any
  HTTP. `<path>` accepts a file or a directory (walked
  recursively); default is `specs/` in the cwd. Default table
  shows `STATUS / FEATURES / SCENARIOS / PATH` per file with an
  indented error line under any `ERR` row; `--json` emits an
  array with `path / status (ok|error) / features / scenarios /
  error`. Exit code is 0 when every file parses, 3 when any
  fails — designed for CI / pre-commit gates. `src/spec.rs`
  gains `collect_feature_paths(dir)` (recursive walker that
  returns sorted paths without loading each file) so the
  validator can iterate without paying the parsing cost
  upfront.
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

[Unreleased]: https://github.com/hazel-ys-lin/serval-cli/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/hazel-ys-lin/serval-cli/releases/tag/v0.3.1
[0.3.0]: https://github.com/hazel-ys-lin/serval-cli/releases/tag/v0.3.0
[0.1.0-alpha.0]: https://github.com/hazel-ys-lin/serval-cli/releases/tag/v0.1.0-alpha.0
