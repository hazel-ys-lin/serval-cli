# serval-cli — Development Guidelines

> Successor to [serval-run-v2](https://github.com/hazel-ys-lin/serval-run-v2)
> (frozen at `v0.2.0`). See the **Origin** section below and the Heptabase
> decision card listed in **Heptabase pointers** before changing direction.

## Project identity

A CLI tool for **spec-anchored API verification**.

- One binary, `servalrun`. Reads `.feature` files from git. Executes them
  against an HTTP target. Reports pass/fail.
- Single-user, no auth, no DB. Specs in git, config in
  `~/.servalrun/config.toml`, results in `.servalrun/reports/<ts>.json`.
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

## Heptabase pointers

All architectural and product decisions live as cards on the
**OSENSE LOGS** whiteboard. Whiteboard ID:
`dda53d6d-6e0b-42f1-92bc-9329618f2dee`.

Read these before substantial changes:

| Card | What it covers |
|---|---|
| `8fabe0aa-f1ba-48cb-8b7c-aead25c81955` | Pivot decision: why `serval-cli` exists |
| `736d1a13-7d2f-4032-bda9-4abc1c088e20` | Positioning: source-agnostic spec execution layer |
| `c507e6ea-9867-494f-9c63-74f3946d915c` | Roadmap (Phase 0-6) |
| `c86059a2-aa29-4e26-aba6-c141b03ce00a` | Why CLI, not MCP |
| `be6dbd69-8841-401c-b253-82e3062453b1` | Spec-as-code (git-first) |
| `93d5a4d1-5184-4950-a49a-0bd7c8acc253` | Phase 3 Mock Server design |
| `4327fbc6-9971-4490-b27c-02b00b5bda29` | Phase 4 Agent Eval Harness |
| `c89c8118-36ab-4155-b5f1-4451a668b77c` | spec for-agent compressed format RFC |
| `62e1fbe1-b696-4ee6-a838-e54bd1154308` | Phase 5 LLM-augmented spec authoring |
| `0ab12828-f25c-4f36-aa01-0ef407c5ba2d` | Phase 4 threat model |
| `d77318bf-2bdf-4984-be6d-3fb2f97955f2` | Live progress tracker (the only card that needs updating; the rest are reference) |

CLI for Heptabase access:

```bash
heptabase note read <cardId>          # full content
heptabase whiteboard cards dda53d6d-6e0b-42f1-92bc-9329618f2dee
```

## Critical rules

### Repo identity

- This is **not** serval-run-v2. Do not reintroduce web-app concepts
  (users, projects, collections, REST handlers, JWT, multi-tenant
  auth) unless an explicit Heptabase decision card says so.
- The CLI binary name is `servalrun` (one word). The repo is
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
  `.servalrun/reports/<ISO-timestamp>.json` in the user's working
  directory.
- Reports are append-only. `servalrun history` lists them, `servalrun
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
- An MCP server. Decision: CLI > MCP for this use case
  (`c86059a2-...` card). Claude Code uses the CLI via Bash.
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

## Learning notes

- Rust learning insights go to the existing Heptabase card on the Rust
  whiteboard. **Do not create a local file.**
  - Card: `Rust 學習筆記：Python/FastAPI 開發者的 Rust 之路`
  - card_id: `fd4f349b-9aaa-48e3-acc0-66716c004505`
  - whiteboard: `986b86a9-fb9d-4fa1-89c0-959ff1002412` (Rust)
- Append command:
  ```bash
  heptabase note append fd4f349b-9aaa-48e3-acc0-66716c004505 \
    --content "## <主題>\n\n<心得內容>"
  ```
- Notes are written from a Python/FastAPI developer's perspective, to
  build mental models for Rust concepts.

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
