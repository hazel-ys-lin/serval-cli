# serval-cli

> Spec-anchored API verification CLI. Run Gherkin `.feature` files against any HTTP target, get pass/fail.

**Status:** v0.3.0 — Phase 0 (lite-native CLI scaffold) and Phase 1 (CLI primary interface) complete. Source-agnostic ingestion (OpenAPI 3.x / AsyncAPI) is on the roadmap; today the CLI consumes Gherkin only.

## What it is

`serval` is a single-binary CLI that reads Gherkin `.feature` files from your repo, executes them against an HTTP target, and reports pass/fail.

- Specs live in git (`specs/*.feature`).
- Config lives in `~/.serval/config.toml`.
- Results land in `.serval/reports/<ISO-timestamp>.json` in the working directory.
- Single-user, no auth, no database.

Three deployment contexts share the same binary:

- **Local development** — quick verification against `localhost`.
- **CI/CD** — green-light builds when contract assertions hold.
- **Agent eval loops** (Claude Code, etc.) — Bash-driven verification of LLM-generated changes.

## What it isn't

- Not a hosted REST service.
- Not a multi-user platform.
- Not a code generator.
- Not a test framework — sits *beside* `cargo test` / `pytest`, doesn't replace them.
- Not an MCP server. Claude Code drives it via `Bash`.

## Install

### Prebuilt binary (no Rust toolchain needed)

```sh
curl -fsSL https://github.com/hazel-ys-lin/serval-cli/releases/latest/download/serval-cli-installer.sh | sh
```

Drops the `serval` binary into `$CARGO_HOME/bin` (default `~/.cargo/bin`). Make sure that directory is on your `PATH`.

### From source

```sh
cargo install --git https://github.com/hazel-ys-lin/serval-cli --tag v0.3.0
```

### Manual download

Pick the `.tar.xz` matching your OS/arch from the [latest release](https://github.com/hazel-ys-lin/serval-cli/releases/latest), extract the `serval` binary, and place it on your `PATH`.

## Quick start

```sh
# 1. Configure an environment once.
serval env set local --base-url http://localhost:3000 --make-default

# 2. Drop a `.feature` file under specs/.
mkdir -p specs && cat > specs/health.feature <<'EOF'
---
api:
  path: /health
  method: GET
---
Feature: Service is up
  Scenario: /health returns 200
    Then status should be 200
EOF

# 3. Run it.
serval run specs/health.feature
```

The run writes a JSON report under `.serval/reports/`; list and compare past runs with `serval history` and `serval diff`.

## Subcommand surface (v0.3.0)

```text
serval run <path> [--env NAME | --base-url URL] [--endpoint P] [--method M]
                  [--report-dir DIR] [--no-report] [--json]
serval history       [--limit N] [--report-dir DIR] [--json]
serval diff <before> <after> [--report-dir DIR] [--json]
serval api list      [--dir DIR] [--json]
serval api show <p>  [--dir DIR] [--json]
serval api find <q>  [--dir DIR] [--json]
serval env list / show NAME / set NAME --base-url URL [--make-default] /
            remove NAME            [--config-file PATH] [--json]
serval config path / show          [--config-file PATH] [--json]
serval spec validate [<path>]      [--json]
```

`--json` is global; pass it on any subcommand to switch the output to machine-readable JSON.

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
