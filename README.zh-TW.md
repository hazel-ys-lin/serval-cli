# serval-cli

> 規格錨定的 API 驗證 CLI。把 Gherkin `.feature` 拿來對任何 HTTP 目標跑，回 pass / fail。

> 🇬🇧 English: [README.md](README.md)

**狀態**：v0.5.0 — Phase 0（lite-native CLI 起步）、Phase 1（CLI 主介面）、Phase 2（step-pattern 引擎：內建 + 使用者 TOML patterns、多步驟 `Action::HttpRequest`、doc-string 深度比對、failure-mode `operation fails with` 斷言、嚴格模式擋空斷言）、Phase 3（codegen Gherkin → REST 翻譯：`DocStringTemplate { rename, defaults, overrides }` 改 body shape、`AssertBodyMatchesAt` 子文件比對、`accepted_status` seed POST idempotency、stream-id 符號表 via 模板化 `capture_response` + 多-pass 模板取代、`doc_captures` 把 doc-string 欄位拉出來串 UUID 鏈）全部完成。Prebuilt binary 透過 [`cargo-dist`](https://github.com/axodotdev/cargo-dist) 出。Source-agnostic 攝取（OpenAPI 3.x / AsyncAPI）在 roadmap；今天 CLI 只吃 Gherkin。

## 它是什麼

`serval` 是單一 binary 的 CLI — 讀你 repo 內的 Gherkin `.feature` 檔、執行對 HTTP 目標的呼叫、回傳 pass / fail。

- **後端語言無關** — serval-cli 只講 HTTP。你的服務寫什麼語言都行（Python、Go、Node、Java、Ruby …），serval-cli 不在意
- 規格放在 git（`specs/*.feature`）
- 設定在 `~/.serval/config.toml`
- 結果落地 `.serval/reports/<ISO-timestamp>.json`（工作目錄下）
- 單一使用者、無 auth、無 DB

三種使用情境共用同一個 binary：

- **本地開發** — 對 `localhost` 快速驗證
- **CI / CD** — 契約斷言通過才放行 build
- **Agent eval loop**（Claude Code 等）— Bash 驅動地驗證 LLM 改動

## 它不是什麼

- 不是託管 REST 服務
- 不是多使用者平台
- 不是 code generator
- 不是 test framework — 在你既有的 test runner 旁邊（pytest / jest / go test / JUnit / cargo test / …）跑，不取代它
- 不是 MCP server — Claude Code 透過 `Bash` 呼叫

## 安裝

### Prebuilt binary（推薦）

```sh
curl -fsSL https://github.com/hazel-ys-lin/serval-cli/releases/latest/download/serval-cli-installer.sh | sh
```

安裝到 `~/.local/bin`（不存在就會建立）。確認該目錄在 `PATH`：

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc  # 或 ~/.bashrc
```

### 手動下載

到 [最新 release](https://github.com/hazel-ys-lin/serval-cli/releases/latest) 抓對應 OS / arch 的 `.tar.xz`，解開把 `serval` binary 放進 `PATH`。

### 從 source 編

Binary 本身是用 Rust 寫的；如果你想自己編譯，先裝 [Rust](https://rustup.rs)，然後：

```sh
cargo install --git https://github.com/hazel-ys-lin/serval-cli --tag v0.5.0
```

## 三分鐘上手

```sh
# 1. 設定一次環境
serval env set local --base-url http://localhost:3000 --make-default

# 2. 丟一個 .feature 在 specs/ 下
mkdir -p specs && cat > specs/health.feature <<'EOF'
---
api:
  path: /health
  method: GET
---
Feature: Service is up
  Scenario: /health 回 200
    Then status should be 200
EOF

# 3. 跑
serval run specs/health.feature
```

跑完寫一份 JSON 報告到 `.serval/reports/`；用 `serval history` 列、`serval diff <id1> <id2>` 比對兩次 run 的 PASS↔FAIL flip。

## Subcommand 介面（v0.5.0）

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

`--json` 是 global flag — 加在任何 subcommand 後面把輸出切成 machine-readable JSON。

## Pattern 引擎（Phase 2 + Phase 3）

`serval run` 透過 **step-pattern 引擎** 把 Gherkin step text 翻譯成 HTTP request + 斷言。兩層：

- **內建 patterns**（編進 binary）— 處理 generic HTTP-shape Gherkin（`Then status should be N`、`Then response contains "x"`、`Given I set header Key Value` …）
- **使用者 patterns** — 疊在上面，從 `~/.serval/patterns.toml`（全域）或 `<repo>/.serval/patterns.toml`（專案）或 `--patterns-file <path>` 指定任意路徑載入。把你團隊的 Gherkin 慣例對映到後端真實 HTTP 形狀：
  - **Body reshape** — `rename` 改鍵名（Gherkin `username` → 你後端 `account`）、`defaults` 補後端必填但 Gherkin 沒提供的欄位、`overrides` 強制覆蓋 doc-string 內的值
  - **Sub-document 比對** — `assert_body_matches_at pointer = "/users"` 對 response 的 JSON 子節點做 partial match，解決 `{users:[...]}` 包裹 vs Gherkin bare `[...]` 的形狀差異
  - **Seed POST idempotency** — `accepted_status = [201, 409]` 容忍 stateful backend 在跨 scenario 撞「資源已存在」
  - **Stream-id ↔ 後端 UUID 符號鏈** — 模板化 `capture_response = { "user_for_{{stream}}" = "/id" }` 抓 UUID 進 scenario 變數；`doc_captures = { team_stream = "/teamId" }` 把 doc-string 內 stream id 拉出來；後續 step 用 `{{$user_for_{{stream}}}}` 多-pass 模板取代到真實 UUID

範例 pattern set 看 [`examples/event-sourcing.toml`](examples/event-sourcing.toml) — 對 event-sourcing 慣例後端（`POST /streams/{id}/events/{Event}`、`POST /streams/{id}/commands/{Cmd}`、`GET /views/{View}`）的完整對映。完整 Phase 3 TOML schema 看 [CHANGELOG.md](CHANGELOG.md#050---2026-05-13)。

### 使用 patterns.toml 的完整範例

```sh
serval run specs/orders.feature \
  --base-url http://localhost:8000 \
  --patterns-file tests/serval/patterns.toml \
  --header "Authorization: Bearer $TOKEN" \
  --endpoint /placeholder --method GET    # 當 .feature 沒 frontmatter 時的 fallback 路徑
```

## Exit code（公開 API）

| Code | 意義 |
|------|------|
| `0` | 操作完成；所有斷言通過 |
| `1` | 操作完成；某個 test 或 spec 斷言失敗 |
| `2` | 系統錯誤（network / auth / config / IO） |
| `3` | 輸入不對（URL 無效、Gherkin 格式錯、缺參數） |

CI 腳本和 agent eval loop 應該對這些 code 分支處理。

## 整合進開發流程的建議

| 階段 | 強制度 | 怎麼做 |
| --- | --- | --- |
| 本地（pre-push） | 建議 | git pre-push hook 或 `make spec-check`；docker compose up + `serval run` |
| **PR / pre-merge CI** | **強制** | GitHub Actions：起後端 stack → alembic / migration → 跑 serval → exit 1/3 擋 merge |
| **部署前（stage / prod）** | **強制** | 對 staging 再跑一次（catch build 後 + env config 差異） |

CI 範例（YAML 雛型）：

```yaml
on: [pull_request]
jobs:
  spec-conformance:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: docker compose up -d --wait
      - run: |
          curl -fsSL https://github.com/hazel-ys-lin/serval-cli/releases/latest/download/serval-cli-installer.sh | sh
      - run: |
          serval run specs/ \
            --base-url http://localhost:8000 \
            --patterns-file tests/serval/patterns.toml \
            --report-dir reports
      - if: always()
        uses: actions/upload-artifact@v4
        with: { name: serval-report, path: reports/*.json }
```

## License

MIT — 見 [LICENSE](LICENSE)。
