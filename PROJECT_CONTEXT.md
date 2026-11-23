# PROJECT_CONTEXT

> Read this first at the start of every session, then verify the real repository state; this file can be wrong.

## Objective
Incident Triage & Release Readiness AI Agent: a Rust CLI orchestrator that uses Claude (Messages API) and two MCP servers (custom Rust `ops-telemetry-mcp` + official filesystem server) to investigate simulated incidents, check release readiness, and create incident tickets only after human approval. Spec: `Intern Assignment 2.pdf` + master instructions + phase prompts.

## Location & Git
`C:\Users\ASUS\Music\SpilLabs\Agent MCP Ochestra` (name kept as-is by user decision).
Own git repo since Phase 1 (`git init -b main`; `git rev-parse --show-toplevel` = this folder). Phase 1 committed on `main` (5 commits, including `package-lock.json`). The author identity is set for this repo only (`ackaashg`, the user's account email, by the user's choice). Never use the repo at `C:\Users\ASUS`.

## Phase status
| Phase | Name | Status |
|---|---|---|
| 0 | Analysis & architecture planning | COMPLETE (approved 2026-11-23) |
| 1 | Environment & project foundation | COMPLETE (approved 2026-11-23; API key deferred by the user) |
| 2 | Mock data engineering | COMPLETE (approved 2026-11-23) |
| 3 | Custom telemetry MCP server | COMPLETE (approved 2026-11-23) |
| 4 | Filesystem MCP integration | IN PROGRESS |
| 5 | Multi-server agent orchestrator | NOT STARTED |
| 6 | Guardrails & security | NOT STARTED |
| 7 | Evaluation & testing | NOT STARTED |
| 8 | Documentation & submission | NOT STARTED |

**Current phase:** 4, IN PROGRESS (approved to start 2026-11-23).
**Next approved action:** filesystem MCP integration. Phase 5 starts only after the user approves Phase 4.

## Phase 3 summary (telemetry MCP server)
- Crate `mcp-server` is now **lib + thin bin**: `lib.rs` (modules), `main.rs` (config → server → `rmcp::transport::stdio()`).
- Modules: `server.rs` (rmcp tools/resources, thin), `data.rs` (service catalogue, typed records, filtering, windows), `tickets.rs` (validation + SQLite), `error.rs` (`{"error": {code, message, hint, valid_values, field_errors}}`), `config.rs` (`OPS_REFERENCE_TIME` validated; `now` = real clock).
- Tools: `search_logs(service_name, severity?, limit?=20 ≤100)`, `get_deployment_history(service_name, window_hours 1..=720)`, `create_incident_ticket(title 5-120, severity LOW|MEDIUM|HIGH|CRITICAL, summary 20-4000, assignee ∈ {identity,payments,messaging}-oncall)`. Annotations: the two reads are `readOnlyHint=true`; ticket `readOnlyHint=false, idempotentHint=false`.
- Resources: `config://release-checklist`, `service://runbooks/{service_name}` (template + 3 concrete URIs listed). MIME `text/markdown`.
- Ticket IDs: `INC-` + (1000 + SQLite AUTOINCREMENT id) → INC-1001, …; `created_at` = real UTC clock; status `OPEN`. DB opened per call.
- Demo: `scripts\demo-mcp.ps1` (`-Raw` for JSON) → raw JSON-RPC to the binary; tickets go to `data\demo-tickets.db`.
- rmcp 3.11.0 compile probe: passed on first build (no downgrade needed).

## Phase 2 summary (mock data)
- `mock_data/`: logs.json (45 entries: INFO 21, WARN 9, ERROR 13, FATAL 2; auth 12 / payments 22 / notification 11), deployments.json (8, one rolled back), release-checklist.md (RC-1..RC-9), runbooks ×3, workspace/{RELEASE_NOTES.md, service-config.yaml}.
- Scenarios: A payments-api outage after 2.14.0 (09:05 → first error 09:12 → FATAL 09:41); B auth key-rotation errors not related to a deployment; C notification-worker 3.2.0 NOT READY (RC-1, 2, 3, 6, 7 fail); D injection in log-0033 (customer_reference field).
- Spec + ground truth: `docs/MOCK_DATA_SPEC.md` (kept outside `mock_data/` so the agent can't read the answers).
- Ticket severities used in the runbooks: LOW / MEDIUM / HIGH / CRITICAL → this becomes the Phase 3 enum for `create_incident_ticket.severity`. Assignees are team handles (`*-oncall`).
- Validation: `mcp-server/tests/mock_data.rs` (16 tests). New test-only dev-deps: serde_json, chrono, yaml-rust2 0.13.2.

## Confirmed decisions
| Topic | Decision | Source |
|---|---|---|
| Language | Rust (server + orchestrator) | master |
| LLM | Anthropic Messages API, raw HTTPS via reqwest (no official Rust SDK) | Phase 0 |
| Model | `claude-opus-5-5` default (`ANTHROPIC_MODEL`), effort `medium`, refusal fallbacks on (switchable) | Phase 0 |
| Key | `.env`; `scripts\setup.ps1` asks for it with hidden input and writes UTF-8 without a BOM | user + Phase 1 |
| Repo | own git repo here; the user creates the GitHub repo; we add the remote in Phase 8 | user |
| Layout | root Cargo workspace; tests inside each crate; `data/ evals/ scripts/ docs/ traces/samples/` | Phase 0 |
| Interface | interactive CLI; web UI only after the CLI is approved | master |
| Filesystem | read-only agent: `--read-only`, root = `mock_data/workspace` | user |
| Approval | Approve / Edit / Reject; edit → re-approval; hash-bound | user |
| Memory | session-only; `/reset` clears; nothing saved between runs | user |
| Teaching notes | `docs/learning/phase-N.md` | user |
| MCP SDK | `rmcp =3.11.0` | Phase 0 |
| Transport | stdio for both servers | Phase 0 |
| Tickets | SQLite (`rusqlite`, bundled) at `data/tickets.db` | Phase 0 |
| FS server | `@modelcontextprotocol/server-filesystem@2.4.0`; **requires Node ≥ 24**; the user chose to upgrade Node (not downgrade the server) | Phase 1 decision |
| Time anchor | seed data around 2026-11-20T12:00:00Z; `OPS_REFERENCE_TIME` | Phase 0 |
| Config loading | only `<cwd>\.env` (no parent search); real env vars override `.env`; parsing via lookup fn (testable) | Phase 1 |
| Errors | `thiserror` enums per module; `anyhow` + context in `main`; no `unwrap` in app code (clippy) | Phase 1 |
| Logging | `tracing` → **stderr**, no ANSI; `LOG_LEVEL` (agent default `warn`, server `info`), `RUST_LOG` overrides; server denies `println!` (clippy) | Phase 1 |
| Assignment PDF | git-ignored (company document) | Phase 1 |
| reqwest TLS | decided in Phase 5 (rustls → aws-lc-rs may need CMake/NASM; native-tls/SChannel needs none) | Phase 1 |
| Server crate shape | library + thin binary (testable in-process and as an executable) | Phase 3 |
| Tool errors | `isError: true` result with `{"error": {code, message, hint, valid_values, field_errors}}`; resources use JSON-RPC errors carrying the same body in `data` | Phase 3 |
| Optional args | `search_logs.severity` (omit/null = all) and `limit` (default 20, max 100) are optional; everything else is required | Phase 3 |
| Service names in schema | free string + description (not an enum), so unknown services reach the server's controlled error (benchmark B4) | Phase 3 |
| Ticket assignee | enum of owning teams' on-call handles, enforced by the server too | Phase 3 |
| Deployment window | include a deployment if it was deployed **or rolled back** inside the window; window 1–720 h | Phase 3 |
| Data loading | files read per request (small); missing/malformed → structured error, no crash, no restart needed | Phase 3 |
| Ticket time | `created_at` = real UTC clock (the reference time only anchors queries) | Phase 3 |

Full design: `docs/PHASE_0_DESIGN.md`.

## Directory structure (actual, end of Phase 3)
```
Cargo.toml  Cargo.lock  clippy.toml  package.json  package-lock.json  .npmrc
.env.example  .gitignore  .gitattributes  .vscode/{extensions,settings}.json
README.md  PROJECT_CONTEXT.md
agent-client/  Cargo.toml  src/{main,config,logging}.rs  tests/cli_smoke.rs
mcp-server/    Cargo.toml  src/{lib,main,config,logging,error,data,tickets,server}.rs
               tests/{startup,mock_data,mcp_protocol}.rs
mock_data/     logs.json deployments.json release-checklist.md runbooks/*.md workspace/{RELEASE_NOTES.md,service-config.yaml}
data/.gitkeep   evals/results/.gitkeep   traces/samples/.gitkeep
scripts/{setup,run,demo-mcp}.ps1
docs/PHASE_0_DESIGN.md  docs/MOCK_DATA_SPEC.md  docs/learning/phase-{0,1,2,3}.md
.env (local only, placeholder key; git-ignored)
```

## Verified environment (2026-11-23)
- rustc / cargo 1.99.1 (stable-x86_64-pc-windows-msvc), with clippy and rustfmt
- Visual Studio Build Tools 2026 (VC tools x64)
- node **v26.13.1** (the user upgraded from 22.18 during Phase 1), npm/npx 11.x
- git 2.51.0; PowerShell 5.1.26100; execution policy CurrentUser = RemoteSigned
- VS Code 1.147.0; Rust extensions not installed (recommended via `.vscode/extensions.json`)
- gh CLI not installed (not needed)

## Dependency versions
In use (Cargo.lock): anyhow 1.0.105 · thiserror 2.0.21 · dotenvy 0.15.7 · tracing 0.1.44 · tracing-subscriber 0.3.23 · tempfile 3.27.0 (dev) · yaml-rust2 0.13.2 (dev).
mcp-server since Phase 3: rmcp 3.11.0 (server, macros, schemars, transport-io; dev: client, transport-child-process) · tokio 1.54.1 · serde 1.0.229 · serde_json 1.0.154 · schemars 1.2.3 · rusqlite 0.43.0 (bundled, libsqlite3-sys 0.40.0) · chrono 0.4.45.
Declared for later phases (not compiled yet): reqwest 0.13.5 · clap 4.6.9 · sha2 0.11.1.
npm (pinned + `package-lock.json` committed, 12 packages): @modelcontextprotocol/server-filesystem 2.4.0.

## Verified commands (run from the project folder)
| Command | Result (2026-11-23) |
|---|---|
| `cargo build --workspace` | OK |
| `cargo test --workspace` | 79 passed, 0 failed (Phase 3) |
| `cargo clippy --workspace --all-targets -- -D warnings` | OK, no warnings |
| `cargo fmt --all -- --check` | OK |
| `powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1 -SkipKey` | exit 1 as expected: open item "Node 24+"; the build and `.env` creation worked |
| `.\scripts\run.ps1` (placeholder key) | friendly error, exit 1 |
| `.\scripts\run.ps1` (key from the environment) | foundation check printed, key shown as `***`, exit 0 |
| `git check-ignore -v .env target data/tickets.db …` | all ignored; `.env.example` not ignored |

| `npm ci` (Node 26.13.1) | OK, 12 packages, server-filesystem 2.4.0 |
| `node node_modules\@modelcontextprotocol\server-filesystem\dist\index.js --read-only "<abs>\mock_data\workspace"` | "Read-only mode" printed; the only allowed dir is `mock_data\workspace`; stdout empty |

**Not verified by me:** the interactive key prompt in `setup.ps1` (needs a human at the keyboard and an API key, which the user doesn't have yet).

## Test results (Phase 3, 2026-11-23)
`cargo test --workspace`: **79 passed, 0 failed**. agent-client 8 unit + 5 cli_smoke; mcp-server 34 unit + 14 mcp_protocol + 16 mock_data + 2 startup. clippy `-D warnings`: clean. `cargo fmt --check`: clean.
- Unit (34): config 4, error 2, data 17 (filters, newest-first, limits, windows, exact names, path traversal, missing and malformed files), tickets 8 (rules, all errors reported together, persistence across instances, sequential IDs, 8 threads × 5 unique, DB errors).
- Protocol (14, compiled binary + rmcp client over stdio): tool discovery (schemas, enums, ranges, additionalProperties=false, annotations), resource and template lists, reads of the checklist and 3 runbooks, hostile or unknown URIs rejected, search_logs (valid, injection returned as data, structured errors), malformed requests (7 cases) with the server surviving, deployment history (versions, commits, timestamps, rollback), invalid windows, tickets (INC-1001/1002, **persisted across a server restart** → INC-1003), injected admin ticket rejected with no row stored, broken DB → DATABASE_ERROR while reads keep working, missing data folder → DATA_UNAVAILABLE.
- Process (2): empty stdin → clean exit 0 with stdout empty; invalid OPS_REFERENCE_TIME → exit 1 with a clear message.
- `scripts\demo-mcp.ps1`: 16 raw JSON-RPC exchanges verified by hand; a second run created INC-1002 (persistence).

Behaviour learned from the SDK (verified by test): rmcp 3.11 reports parameter-deserialisation failures as tool results with `isError: true` (not JSON-RPC -32602).

## Test results (Phase 2, 2026-11-23)
`cargo test --workspace`: **32 passed, 0 failed** (Phase 1's 16 + mock_data 16). clippy `-D warnings`: clean. `cargo fmt --check`: clean.
One defect found and fixed during the phase: the notification-worker runbook lacked a CRITICAL escalation rule.

## Test results (Phase 1)
- agent-client unit (config): 8/8
- agent-client integration (cli_smoke): 5/5. Missing key; key never printed (debug logging on); env overrides `.env`; `.env.example` placeholder-only and parseable; invalid setting named
- mcp-server unit (config): 2/2
- mcp-server integration (startup): 1/1 (stdout stays empty)

## Known issues / open items
- **No API key yet** (the user will get one later). Everything up to Phase 4 works without it (tests need no key). It's needed for the first live agent run in Phase 5; then run `scripts\setup.ps1` (or `-ResetKey`).
- GitHub repo: the user provides the URL in Phase 8.
- Resolved in Phase 3: rmcp 3.11.0 compile probe passed; `OPS_REFERENCE_TIME` validated at server start.
- Phase 5 must relay both error forms to Claude: tool results with `isError: true`, and JSON-RPC errors from `resources/read` (body in `error.data`).

## Security considerations
Design §9. Phase 1 controls in place: `.env` ignored; `.env.example` placeholder-only (tested); key wrapped in `ApiKey` with redacting Debug/Display (tested incl. debug logs); hidden key entry in setup; `.npmrc` ignore-scripts + engine-strict; `unsafe_code` forbidden; stdout of the MCP server reserved for the protocol (lint + test). Phase 3: allow-listed service names before any file path (path traversal tested); ticket assignee allow-list (an injected `admin` ticket is rejected by the server); parameterised SQL + CHECK constraints; error messages hide paths and SQLite details; untrusted-data notice in log results.
