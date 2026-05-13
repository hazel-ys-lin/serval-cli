# serval-cli

> Spec-anchored API verification CLI. Run Gherkin `.feature` files against any HTTP target, get pass/fail.

> 🇹🇼 中文文件：[README.zh-TW.md](README.zh-TW.md)

**Status:** v0.5.0 — Phase 0 (lite-native CLI scaffold), Phase 1 (CLI primary interface), Phase 2 (step-pattern engine: built-in + user TOML patterns, multi-step `Action::HttpRequest`, doc-string deep match, failure-mode `operation fails with` step, strict vacuous-PASS detection), and Phase 3 (codegen Gherkin → REST translation: `DocStringTemplate { rename, defaults, overrides }` body reshape, `AssertBodyMatchesAt` scoped deep-match, `accepted_status` seed idempotency, stream-id symbol table via templated `capture_response` + multi-pass template substitution, `doc_captures` for body-field UUID chains) complete. Prebuilt binaries ship via [`cargo-dist`](https://github.com/axodotdev/cargo-dist). Source-agnostic ingestion (OpenAPI 3.x / AsyncAPI) is on the roadmap; today the CLI consumes Gherkin only.

## What it is

`serval` is a single-binary CLI that reads Gherkin `.feature` files from your repo, executes them against an HTTP target, and reports pass/fail.

- **Backend-agnostic** — serval-cli only speaks HTTP. Your service can be written in any language (Python, Go, Node, Java, Ruby, …); serval-cli doesn't care.
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
- Not a test framework — sits *beside* your existing test runner (pytest / jest / go test / JUnit / cargo test / …), doesn't replace it.
- Not an MCP server. Claude Code drives it via `Bash`.

## Install

### Prebuilt binary (recommended)

```sh
curl -fsSL https://github.com/hazel-ys-lin/serval-cli/releases/latest/download/serval-cli-installer.sh | sh
```

Drops the `serval` binary into `~/.local/bin` (created if it doesn't exist). Make sure that directory is on your `PATH`:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc  # or ~/.bashrc
```

### Manual download

Pick the `.tar.xz` matching your OS/arch from the [latest release](https://github.com/hazel-ys-lin/serval-cli/releases/latest), extract the `serval` binary, and place it on your `PATH`.

### From source

The binary itself is written in Rust; if you'd rather build from source, install [Rust](https://rustup.rs) first then:

```sh
cargo install --git https://github.com/hazel-ys-lin/serval-cli --tag v0.5.0
```

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

## Subcommand surface (v0.5.0)

```text
serval run <path> [--env NAME | --base-url URL] [--endpoint P] [--method M]
                  [--patterns-file PATH] [--header "Key: Value"]…
                  [--allow-no-assertions]
                  [--report-dir DIR] [--no-report] [--json]
                  [--config-file PATH]
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

## Pattern engine (Phase 2 + Phase 3)

`serval run` translates Gherkin step text into HTTP requests + assertions through a **step-pattern engine**. Two tiers:

- **Built-in patterns** ship inside the binary and cover generic HTTP-shape Gherkin (`Then status should be N`, `Then response contains "x"`, `Given I set header Key Value`, …).
- **User patterns** layer on top via `~/.serval/patterns.toml` (global) or `<repo>/.serval/patterns.toml` (project), or any path via `--patterns-file`. They map your team's Gherkin convention to your backend's actual HTTP shape — including body reshape (`rename` / `defaults` / `overrides`), JSON-pointer-scoped deep-match (`assert_body_matches_at`), seed-POST idempotency (`accepted_status`), and stream-id ↔ backend-UUID symbol chains (templated `capture_response` + `doc_captures`).

See [`examples/event-sourcing.toml`](examples/event-sourcing.toml) for a working pattern set against the event-sourcing convention (`POST /streams/{id}/events/{Event}`, `POST /streams/{id}/commands/{Cmd}`, `GET /views/{View}`). The full CHANGELOG entry for Phase 3 ([CHANGELOG.md](CHANGELOG.md#050---2026-05-13)) walks through every TOML schema knob.

## Exit codes (public API)

| Code | Meaning |
|------|---------|
| `0` | Operation completed; assertions passed. |
| `1` | Operation completed; a test or spec assertion failed. |
| `2` | System error (network / auth / config / IO). |
| `3` | Bad input (invalid URL, malformed Gherkin, missing arg). |

CI scripts and agent eval loops should branch on these.

## License

MIT — see [LICENSE](LICENSE).
