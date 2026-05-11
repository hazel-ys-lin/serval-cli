# serval-cli

> Spec-anchored API verification CLI. Run Gherkin `.feature` files against any HTTP target, get pass/fail.

**Status:** early alpha — Phase 0 in progress. The binary scaffold lands one commit at a time; the subcommands listed below are the planned surface, not all functional yet.

## What it is

`servalrun` is a single-binary CLI that reads Gherkin `.feature` files from your repo, executes them against an HTTP target, and reports pass/fail.

- Specs live in git (`specs/*.feature`).
- Config lives in `~/.servalrun/config.toml`.
- Results land in `.servalrun/reports/<ISO-timestamp>.json` in the working directory.
- Single-user, no auth, no database.

Three deployment contexts share the same binary:

- **Local development** — quick verification against `localhost`.
- **CI/CD** — green-light builds when contract assertions hold.
- **Agent eval loops** (Claude Code, etc.) — Bash-driven verification of LLM-generated changes.

Source-agnostic by design: ingests Gherkin today; OpenAPI 3.x and AsyncAPI on the roadmap.

## What it isn't

- Not a hosted REST service.
- Not a multi-user platform.
- Not a code generator.
- Not a test framework — sits *beside* `cargo test` / `pytest`, doesn't replace them.
- Not an MCP server. Claude Code drives it via `Bash`.

## Quick start

```bash
cargo install --path .
servalrun --help
```

Planned subcommand surface:

```bash
servalrun run                 # execute every .feature file under specs/
servalrun run path/to/foo     # execute a specific feature
servalrun history             # list past report files
servalrun diff <a> <b>        # compare two reports
servalrun mock                # serve .feature files as a local mock HTTP server
```

## Exit codes (public API)

| Code | Meaning |
|------|---------|
| `0` | Operation completed; assertions passed. |
| `1` | Operation completed; a test or spec assertion failed. |
| `2` | System error (network / auth / config / IO). |
| `3` | Bad input (invalid URL, malformed Gherkin, missing arg). |

CI scripts and agent eval loops should branch on these.

## Origin

`serval-cli` is the CLI-focused successor to [`serval-run-v2`](https://github.com/hazel-ys-lin/serval-run-v2), frozen at `v0.2.0`.

The earlier project explored a Rust REST service backed by SeaORM, MongoDB, and Redis for storing specs, runs, and reports. Through 2026 design discussions it became clear that the right shape for daily use is a CLI tool reading specs straight from git — not a hosted multi-user service. Industry precedent (Postman → Newman, Vue CLI → Vite, `gh` vs `hub`) was the tiebreaker: when the new tool is conceptually a different product, start a new repo rather than patching the old one.

## License

MIT — see [LICENSE](LICENSE).
