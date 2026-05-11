# serval-cli — Development Guidelines

> Successor to [serval-run-v2](https://github.com/hazel-ys-lin/serval-run-v2)
> (frozen at `v0.2.0`). See the **Origin** section below before changing
> direction.

## Project identity

A CLI tool for **spec-anchored API verification**.

- One binary, `serval`. Reads `.feature` files from git. Executes them
  against an HTTP target. Reports pass/fail.
- Single-user, no auth, no DB. Specs in git, config in
  `~/.serval/config.toml`, results in `.serval/reports/<ts>.json`.
- The optional `mock` subcommand turns the same `.feature` files into a
  local mock HTTP server for frontend / agent consumption.
- Source-agnostic: ingests Gherkin today, OpenAPI 3.x / AsyncAPI next.
- Three deployment contexts share the same binary: local dev, CI/CD,
  agent (Claude Code etc.) eval loops.

It is **not**:

- A web service
- A multi-user platform
- A code generator
- A test framework — sits *beside* `cargo test` / `pytest`, doesn't
  replace them

## Origin

serval-run-v2 was a Rust rewrite of an earlier Node.js web app. Through
2026-05 design discussions it became clear the right shape for daily
work is a CLI tool, not a hosted REST service.

Key turning points:

1. Phase 0 lite mode (SQLite + skip Mongo/Redis) felt like patches on a
   web architecture.
2. Investigating a SQLite UUID rendering issue surfaced a deeper
   question — "do we even need persistent storage for a CLI tool?" —
   answer: no, not for the single-user case.
3. Industry precedent (Postman → Newman, Vue CLI → Vite, `gh` vs `hub`)
   says: when the new tool is conceptually a different product, **start
   a new repo**.

Result: serval-run-v2 frozen at `v0.2.0`; this repo (`serval-cli`) is
the focused CLI successor.

## Critical rules

### Repo identity

- This is **not** serval-run-v2. Do not reintroduce web-app concepts
  (users, projects, collections, REST handlers, JWT, multi-tenant
  auth) unless there is an explicit, documented design decision
  saying otherwise.
- The CLI binary name is `serval`. The repo and package name is
  `serval-cli` (with hyphen). Don't confuse them.

### Specs

- Specs live in `specs/*.feature` in the user's repo, not in this
  repo's DB. There is no DB.
- Path conventions:
  `specs/<area>/<feature>.feature` mirrors `src/handlers/<area>.rs` or
  equivalent in the user's project. `serval-cli` is configurable, not
  prescriptive.

### Results

- A test run produces a JSON report at
  `.serval/reports/<ISO-timestamp>.json` in the user's working
  directory.
- Reports are append-only. `serval history` lists them, `serval
  diff <id>` compares two.

### CLI conventions

- Output formats: human-readable table (default) and `--json` for
  agents / CI.
- Exit codes are **part of the public API**:
  - `0` — operation completed; assertions passed
  - `1` — operation completed; a test or spec assertion failed
  - `2` — system error (network / auth / config / IO)
  - `3` — bad input (invalid URL, malformed Gherkin, missing arg)
- New subcommands live in `src/cli/commands/<name>.rs` and dispatch
  from `src/cli/mod.rs`.
- `clap` derive macros for argument parsing.
- Errors aimed at users should be actionable: name the problem, name
  the fix.

### Don't ship

- A REST server (the `mock` subcommand serves HTTP, but is started by
  the CLI and shuts down with it — not a daemon).
- An MCP server. Decision: CLI > MCP for this use case. Claude Code
  uses the CLI via Bash.
- Code generators. Static codegen per language is brittle; Claude Code
  / users translate to their preferred language.
- An authoring UI. Specs are written in editors; this tool is for
  execution and verification.
- A persistent DB.

## Testing

- `cargo test --lib` runs unit tests; no external services required.
- CI runs fmt, clippy, and tests on every PR (see
  `.github/workflows/ci.yml`).
- Integration tests for CLI commands use `assert_cmd` or similar; spawn
  the binary, assert on stdout / stderr / exit code.

## Code style

- Run `cargo fmt` before `cargo clippy`.
- `-D warnings` in clippy — zero tolerance.
- Comments: default to **none**. Only add when the WHY is non-obvious
  (a hidden constraint, a subtle invariant, a workaround). Don't
  explain WHAT the code does.

## Commits and PRs

- Conventional Commits: `feat(scope): ...`, `fix(scope): ...`,
  `docs: ...`, `chore: ...`, `refactor: ...`, `ci: ...`, `style: ...`.
- Subject ≤ 72 chars. Body explains the why and notable trade-offs.
- Author email: `hazel.ys.lin@gmail.com` (set per-repo).
- All commits include
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.
- Branch naming: `feat/<phase>/<scope>`, `fix/<scope>`,
  `docs/<scope>`. Direct push to `main` after fast-forward merge from
  feature branches is the team norm (single committer).
- Open a draft PR to trigger CI before fast-forward merging when the
  change is non-trivial.

## User preferences

- Communicates in **Traditional Chinese (繁體中文)**.
- Prefers **concise, opinionated** responses. Don't hedge.
- Verifies code locally (cargo isn't installed in Claude's environment);
  always wait for the user's explicit confirmation (e.g. "commit",
  "OK", "push") before committing, pushing, or merging.
- Decision flow: present trade-offs honestly, recommend one, **ask
  before acting** on irreversible actions (commits, pushes, branch
  deletes, releases).
- The work is for **daily use at OSENSE + portfolio**, not a commercial
  product.
