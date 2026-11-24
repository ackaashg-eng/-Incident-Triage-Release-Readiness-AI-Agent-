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
| 4 | Filesystem MCP integration | COMPLETE (approved 2026-11-23) |
| 5 | Multi-server agent orchestrator | COMPLETE (approved 2026-11-24; live Claude run deferred until the account has credits) |
| 6 | Guardrails & security | COMPLETE (approved 2026-11-24) |
| 7 | Evaluation & testing | COMPLETE (approved 2026-11-24; live LLM run still open, see below) |
| 8 | Documentation & submission | COMPLETE (approved 2026-11-24); NOT submission-ready: live Claude run + GitHub remote outstanding |

**Current phase:** all phases approved (8 committed 2026-11-24).
**Next approved action:** none. Waiting for: GitHub URL (add remote, push only when asked); API credits + key → live `eval` run and a live trace, then update EVALUATION.md §5, REPORT.md §11.2/§14 and traces/samples.

## Phase 8 summary (documentation & submission)
- README.md rewritten (overview, architecture, components, prerequisites incl. Rust/Node install, setup, configuration incl. filesystem MCP, running server/agent/CLI/eval/audit, example prompts, tests, troubleshooting, layout).
- REPORT.md: objective, diagram, components, MCP flow, decoupling, discovery, loop, guardrails, injection defence, HITL, evaluation tables, 2 failure-mode reflections, limitations, compliance matrix (R1–R21, M1–M10: Complete except R17/R20/M5 Partial, M10 Pending).
- Traces: `traces/samples/investigation-with-hitl-approval.jsonl` and `handled-failures-and-edge-cases.jsonl` (+ `.console.txt`) from real binary runs with a scripted model endpoint (`api_endpoint` recorded in session_start); provenance in `traces/samples/README.md`; regenerate via `cargo test -p agent-client --test cli_e2e -- --ignored --test-threads=1`.
- `scripts/audit.ps1`: repeatable audit (files, build, check-mcp, tests, clippy, fmt, .env tracking, secret scan of tracked files, sample-trace leak check, git clean, remote).
- Fixes found by the audit: `setup.ps1` ran the interactive agent → now `check-mcp` + `config`; CLI banner leaked the trace path → redacted.

## Phase 7 summary (evaluation)
- Suite `evals/benchmarks.json`: B1–B7 (lookup, RCA, readiness for notification-worker 3.2.0, missing service, invalid severity, ambiguous, injection) + G1/G2 (HITL approve/reject); patterns (`tool`, `a|b`, `input_contains`, `optional`), `forbidden_calls`, `approvals`, `checks`.
- `agent-client eval [--suite F] [--out D] [IDS]` (`src/eval.rs`): live run, fresh conversation per case, `ScriptedApprover.set_steps`, eval-only ticket DB, verdicts PASS/FAIL/ERROR/NOT_RUN, stops after persistent API errors; writes `evals/results/eval-<UTC>/{results.json, report.md, traces.jsonl}`; report has the required table + check details + a separate manual quality section.
- Functional benchmarks `tests/benchmarks_functional.rs` (7, scripted model, real servers; latencies 0–49 ms); harness test `tests/eval_harness.rs`.
- `docs/EVALUATION.md`: suite, method, functional tables (measured), guardrail table, live status (NOT obtained), failure analysis, limitations.
- **Live LLM evaluation: NOT RUN.** 2026-11-24 attempts: credit balance too low (req_011ChLMcWXwsVqGGtx9G8aTn); then `.env` key empty → `ANTHROPIC_API_KEY is not set`. To finish: credits + `setup.ps1 -ResetKey` + `.\scripts\run.ps1 eval` → paste `report.md` into EVALUATION.md §5.

## Phase 6 summary (guardrails)
- Policy (`agent/registry.rs`): explicit `READ_ONLY_POLICY` (2 telemetry + 10 files tools; + generated read_resource) and `APPROVAL_REQUIRED` (create_incident_ticket); anything else blocked; server annotation can only make stricter (conflict → blocked).
- Approval (`agent/approval.rs`): `Proposal` with SHA-256 of `server\ntool\ncanonical JSON`; `ConsoleApprover` (a/r/e, edit → fresh approval, invalid edit can't be approved, EOF = reject) reading the shared stdin (`SharedStdin`, also used by the REPL); `ScriptedApprover` (Approve/Reject/EditThenApprove/Tamper) for tests and evals; `DeferredApprover`. Runner re-hashes before executing → `payload_mismatch_blocked`.
- `safety/injection.rs` (phrase rules → flags; `<tool_output … trust="untrusted-data">` wrapper with SECURITY WARNING; closing-tag neutralised), `safety/redact.rs` (exact key, token patterns, PEM, Bearer, password=, sensitive JSON keys, local paths → `<project>`/`<home>`).
- `trace.rs`: JSONL `traces/agent_traces.jsonl` (events session_start, user_request, model_call, approval, tool_call, warning, turn_end, session_end; every line redacted). `ToolCallRecord` with status + approval enums.
- Per-tool timeouts `TOOL_TIMEOUT_OVERRIDES`; `ServerSet::from_connections`; test fixture `tests/fixtures/misbehaving_mcp_server.js`.
- Docs: `docs/SECURITY.md` (threats, layers, policy, flow, tests table); sample `traces/samples/{README.md, investigation-with-hitl-approval.jsonl/.console.txt, handled-failures-and-edge-cases.jsonl/.console.txt, phase6-scripted-security-demo.jsonl}` (scripted model, labelled), regenerate with `cargo test -p agent-client --test security -- --ignored`.

**Live-API verification deferred (user decision 2026-11-24):** the key in `.env` authenticates, but the Anthropic account has no credits (400 `credit balance is too low`, request req_011ChLMcWXwsVqGGtx9G8aTn). The user said: build on as if the key works. Every phase is verified offline (scripted Claude / local fake API). **Before submission (Phase 7/8), once credits exist:** run the live demo, `.\scripts\run.ps1 ask "Customers report failed payments since this morning. Investigate payments-api and open an incident ticket if warranted."`, and the evaluation suite. Grok/other providers: discussed, not adopted (master prompt mandates Claude; multi-provider support would be an optional later extension).

## Phase 5 summary (agent orchestrator)
- `agent-client` modules: `llm/` (`types.rs` request/response, `anthropic.rs` HTTP + retries + `LlmClient` trait), `agent/` (`registry.rs` discovery → namespaced tools + permission + schema adaptation, `validate.rs` JSON-schema subset validator, `prompt.rs` system prompt without tool names, `approval.rs` `Approver` trait + `DeferredApprover`, `runner.rs` agentic loop + `Observer` events), `mcp/manager.rs` (`ServerSet`: concurrent connect, discovery, graceful degradation, shutdown), `cli.rs` (interactive REPL + `ask`).
- CLI: `agent-client` (interactive: /tools /servers /reset /help /exit, Ctrl+C cancels a request), `ask "..."`, `check-mcp`, `config` (old foundation check).
- Tool names `<server>__<tool>`; generated `telemetry__read_resource`; 14 tools (3 + 1 telemetry, 10 files); policy: readOnlyHint → read-only, `telemetry/create_incident_ticket` → requires approval, else blocked.
- Loop: max_iterations; all tool_results in one user message; refusal/max_tokens → no tools run, history rolled back; API error → rollback; iteration limit → synthetic assistant note keeps alternation.
- Request: model, max_tokens 16000, cached system block, tools sorted, tool_choice auto, output_config.effort, thinking `{adaptive, display: updates}` when progress updates on, fallbacks `default`; betas `server-side-fallback-2026-07-01,thinking-display-updates-2026-08-18`; anthropic-version 2026-10-21.
- New config: `ANTHROPIC_PROGRESS_UPDATES` (on), `ANTHROPIC_TIMEOUT_SECS` (300), `ANTHROPIC_BASE_URL` (tests only; https or localhost), `TICKETS_DB`.

## Phase 4 summary (filesystem MCP integration)
- External server unchanged: `node <project>\node_modules\@modelcontextprotocol\server-filesystem\dist\index.js --read-only --confine-roots -- <project>\mock_data\workspace` (no npx; arguments as a list; cwd = workspace).
- `agent-client` is now **lib + bin**. New `src/mcp/`: `connection.rs` (generic stdio client: spawn, handshake timeout, per-request timeout, stderr tail, `McpError` {Spawn, StartupTimeout, Handshake, Timeout, Disconnected, Rejected, Request, Unsafe}, shutdown), `filesystem.rs` (pre-launch workspace checks, post-connect verification: all tools readOnlyHint + exactly one allowed dir), `telemetry.rs` (launch of ops-telemetry-mcp with DATA_DIR/TICKETS_DB/OPS_REFERENCE_TIME), `child_env.rs` (env_clear + 17 allow-listed system vars; API key never reaches children). `src/check.rs` = `agent-client check-mcp`.
- Config split: `McpSettings` (no API key needed) inside `Config.mcp`; new optional vars `MCP_STARTUP_TIMEOUT_SECS` (20), `NODE_EXE` (node), `FILESYSTEM_SERVER_SCRIPT`.
- Docs: `docs/FILESYSTEM_MCP.md` (install/launch, controls, discovered capabilities, test documentation, limitations).

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
| FS launch | `node` + locked local script (not npx); `--read-only --confine-roots --`; workspace only | Phase 4 |
| Child env | every MCP server starts with `env_clear()` + system allow-list (no API key, no NODE_OPTIONS) | Phase 4 |
| FS verification | fail closed: refuse if any tool lacks `readOnlyHint: true` or allowed dirs ≠ workspace; pre-launch scan refuses secrets/links in the workspace | Phase 4 |
| `check-mcp` | `agent-client check-mcp` / `.\scripts\run.ps1 check-mcp` verifies both servers without an API key | Phase 4 |
| reqwest TLS (final) | `native-tls` (Windows SChannel); no CMake/NASM needed | Phase 5 |
| Tool naming | `<server>__<tool>`; resources via generated `<server>__read_resource` (only for servers that list resources) | Phase 5 |
| Permission policy | readOnlyHint → ReadOnly; explicit list → RequiresApproval; else Blocked (not sent to Claude) | Phase 5 |
| Argument validation | client-side against the ORIGINAL MCP schema before any call; servers validate again | Phase 5 |
| Tool result to Claude | text content (structured only if no text); 30,000-char cap | Phase 5 |
| Approval hand-off | `Approver` trait; Phase 5 `DeferredApprover` = shown, never executed | Phase 5 |
| Child process group | MCP servers start with CREATE_NEW_PROCESS_GROUP so Ctrl+C cancels a request without killing them | Phase 5 |
| Mode names | no args = interactive agent; foundation check moved to `agent-client config` | Phase 5 |
| Permission source | application allow-list (not tool descriptions/annotations); annotations may only restrict | Phase 6 |
| Approval binding | SHA-256(server, tool, canonical args); executed hash must equal approved hash | Phase 6 |
| Approval input | console only (shared stdin); EOF = reject; edit → fresh approval | Phase 6 |
| Tool output to Claude | redacted → injection-scanned → wrapped as untrusted-data (+ warning if flagged) | Phase 6 |
| Trace | JSONL, append, every line redacted; tests use their own TRACE_FILE | Phase 6 |
| B3 subject | notification-worker 3.2.0 (designed release candidate), not payments (2.14.0 is already deployed) | Phase 7 |
| Eval verdicts | PASS/FAIL functional checks only; ERROR when the API call failed; answer quality = manual section | Phase 7 |

Full design: `docs/PHASE_0_DESIGN.md`.

## Directory structure (actual, Phase 8)
```
Cargo.toml  Cargo.lock  clippy.toml  package.json  package-lock.json  .npmrc
.env.example  .gitignore  .gitattributes  .vscode/{extensions,settings}.json
README.md  REPORT.md  PROJECT_CONTEXT.md
agent-client/  Cargo.toml  src/{lib,main,config,logging,check,cli}.rs
               src/mcp/{mod,connection,filesystem,telemetry,child_env,manager}.rs  src/llm/{mod,types,anthropic}.rs
               src/agent/{mod,registry,validate,prompt,approval,runner}.rs  src/safety/{mod,injection,redact}.rs  src/trace.rs  src/eval.rs
               tests/{cli_smoke,cli_e2e,filesystem_mcp,agent_loop,anthropic_http,security,benchmarks_functional,eval_harness}.rs  tests/common/mod.rs  tests/fixtures/misbehaving_mcp_server.js
traces/samples/{README.md, investigation-with-hitl-approval.jsonl/.console.txt, handled-failures-and-edge-cases.jsonl/.console.txt, phase6-scripted-security-demo.jsonl}
mcp-server/    Cargo.toml  src/{lib,main,config,logging,error,data,tickets,server}.rs
               tests/{startup,mock_data,mcp_protocol}.rs
mock_data/     logs.json deployments.json release-checklist.md runbooks/*.md workspace/{RELEASE_NOTES.md,service-config.yaml}
data/.gitkeep   evals/results/.gitkeep   traces/samples/.gitkeep
scripts/{setup,run,demo-mcp,audit}.ps1
docs/PHASE_0_DESIGN.md  docs/MOCK_DATA_SPEC.md  docs/FILESYSTEM_MCP.md  docs/SECURITY.md  docs/EVALUATION.md  docs/learning/phase-{0,1,2,3,4,5,6,7,8}.md
evals/benchmarks.json  evals/results/.gitkeep
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
| `cargo test --workspace` | 197 passed, 0 failed, 2 ignored opt-in generators (Phase 8) |
| `scripts\audit.ps1` | after the Phase 8 commit: see the latest run (2026-11-24) |
| `scripts\setup.ps1 -SkipKey` | exit 0 (check-mcp all ok) on 2026-11-24 |
| `.\scripts\run.ps1 eval` | not completed: `ANTHROPIC_API_KEY is not set` (2026-11-24) |
| `.\scripts\run.ps1 check-mcp` | all checks passed; no node.exe left running |
| `cargo clippy --workspace --all-targets -- -D warnings` | OK, no warnings |
| `cargo fmt --all -- --check` | OK |
| `powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1 -SkipKey` | exit 1 as expected: open item "Node 24+"; the build and `.env` creation worked |
| `.\scripts\run.ps1` (placeholder key) | friendly error, exit 1 |
| `.\scripts\run.ps1` (key from the environment) | foundation check printed, key shown as `***`, exit 0 |
| `git check-ignore -v .env target data/tickets.db …` | all ignored; `.env.example` not ignored |

| `npm ci` (Node 26.13.1) | OK, 12 packages, server-filesystem 2.4.0 |
| `node node_modules\@modelcontextprotocol\server-filesystem\dist\index.js --read-only "<abs>\mock_data\workspace"` | "Read-only mode" printed; the only allowed dir is `mock_data\workspace`; stdout empty |

**Not verified by me:** the interactive key prompt in `setup.ps1` (needs a human at the keyboard and an API key, which the user doesn't have yet).

## Test results (Phase 7, 2026-11-24)
`cargo test --workspace`: **197 passed, 0 failed** (+1 ignored), run twice; clippy + fmt clean. New: benchmarks_functional 7 (B1 8 ms, B2 49, B3 42, B4 11, B5 1, B6 0, B7 21), eval_harness 1, eval unit 4. Live benchmark verdicts: none (see Phase 7 summary). Details: docs/EVALUATION.md.

## Test results (Phase 6, 2026-11-24)
`cargo test --workspace`: **185 passed, 0 failed** (+1 ignored sample-trace writer), run twice; no leftover processes; real trace file untouched. clippy `-D warnings` + fmt clean. New: security.rs 14 (approved/rejected/edited/tampered tickets, invalid write payloads never reach the reviewer, injection in logs with a fooled model, injection + secrets in files, 10 traversal paths, unauthorized tools, default 8-iteration limit, hanging tool → timeout, server death → disconnected, API failure traced + rollback, trace fields + redaction); cli_e2e +3 (console approve / reject / EOF through the real binary); unit +22 (policy, approval incl. console edit flow, hash, injection rules incl. 'only log-0033 flagged', redaction incl. no false positives on mock data, trace format, timeouts parsing). Full mapping: docs/SECURITY.md §6.

## Test results (Phase 5, 2026-11-24)
`cargo test --workspace`: **145 passed, 0 failed**, run twice; no node/telemetry processes left. clippy `-D warnings` + fmt clean. agent-client: 37 unit, 8 agent_loop, 8 anthropic_http, 4 cli_e2e, 7 cli_smoke, 15 filesystem_mcp; mcp-server 66.
- agent_loop (scripted Claude + REAL servers): both servers connect, 14 tools mapped to the right server; full investigation (parallel search_logs + get_deployment_history → runbook resource + RELEASE_NOTES via files → ticket PROPOSAL not executed → answer), identical tools/system in every request, verbatim history; unknown tool / invalid args / unknown service / outside path / hostile URI / missing resource / bad window all returned to Claude as errors; refusal + max_tokens run no tools and roll back; API error rollback; iteration limit; malformed responses; reset.
- anthropic_http (real client vs local fake API): headers (key, version 2026-10-21, betas), body rules, 429/529 retried with retry-after, overload/server errors give up after 3 retries, 401/403/400/404 not retried, malformed JSON, timeouts, unreachable endpoint; key never in errors.
- cli_e2e (real binary + real servers + fake API): `ask` multi-step investigation output and API payloads, interactive commands//reset//exit, 401 reported with setup hint, usage error.
- **Not yet verified: a live run against the real Claude API** (no key).

## Test results (Phase 4, 2026-11-23)
`cargo test --workspace`: **104 passed, 0 failed**, run twice. agent-client 16 unit + 7 cli_smoke + 15 filesystem_mcp; mcp-server 34 + 14 + 16 + 2. clippy `-D warnings` and fmt: clean.
- filesystem_mcp (real Node server): startup/connection, exactly 10 read-only tools, `file://{+path}` template + empty resource list, allowed dir = workspace, both files read (relative, absolute with spaces, `file://` resource), 13 outside paths + 4 directory tools + outside resource all `Access denied`, 6 write tools "not found" and nothing written, missing file (ENOENT, connection still fine), shutdown (PID gone), `taskkill` → `Disconnected` fast, missing node.exe, missing package, unsafe workspace, non-MCP program (handshake error with stderr), silent program (startup timeout), scrubbed env.
- Learned: the Node server omits `isError` on success (absent = success) → always test `== Some(true)`. Negotiated protocol 2025-11-25.

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
- **No API key yet** (placeholder in `.env` on 2026-11-24). All tests run without it. It's needed for the Phase 5 live demo: `.\scripts\setup.ps1 -ResetKey` → paste → `.\scripts\run.ps1 ask "Investigate the payments-api errors"`.
- Live-only unknowns until the key exists: acceptance of the `thinking-display-updates` beta (switch off with `ANTHROPIC_PROGRESS_UPDATES=off` if a 400 names it) and real token/latency numbers.
- GitHub repo: the user provides the URL in Phase 8.
- Resolved in Phase 3: rmcp 3.11.0 compile probe passed; `OPS_REFERENCE_TIME` validated at server start.
- Resolved in Phase 6: absolute paths in tool output/traces are shown as `<project>`/`<home>`.
- Phase 5 must relay both error forms to Claude: tool results with `isError: true`, and JSON-RPC errors from `resources/read` (body in `error.data`).

## Security considerations
Design §9. Phase 1 controls in place: `.env` ignored; `.env.example` placeholder-only (tested); key wrapped in `ApiKey` with redacting Debug/Display (tested incl. debug logs); hidden key entry in setup; `.npmrc` ignore-scripts + engine-strict; `unsafe_code` forbidden; stdout of the MCP server reserved for the protocol (lint + test). Phase 3: allow-listed service names before any file path (path traversal tested); ticket assignee allow-list (an injected `admin` ticket is rejected by the server); parameterised SQL + CHECK constraints; error messages hide paths and SQLite details; untrusted-data notice in log results. Phase 4: filesystem server read-only + confine-roots + single dir, verified after connect (fail closed); pre-launch workspace scan; children get a scrubbed environment (API key never inherited); per-request and startup timeouts; documented upstream Windows junction race (docs/FILESYSTEM_MCP.md §7).
