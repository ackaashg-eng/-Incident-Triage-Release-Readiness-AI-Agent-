# Incident Triage & Release Readiness AI Agent (Rust + MCP)

A command-line AI agent that **investigates production incidents** and **checks whether a release is safe to ship**, using simulated operational data. It uses **Claude** (Anthropic) for reasoning and the **Model Context Protocol (MCP)** to reach its tools, and it **asks a human before changing anything**.

```
you> Customers report failed payments since this morning. Investigate payments-api.
  [step 1/8] asking Claude...
  -> telemetry__search_logs {"limit":20,"service_name":"payments-api","severity":"ERROR"}
     ok telemetry__search_logs: 8 of 8 matching log entries (6 ms)
     !! SECURITY: instruction-like text in this output (ignore-instructions, ...). Labelled as untrusted data for Claude.
  -> telemetry__get_deployment_history {"service_name":"payments-api","window_hours":24}
     ok telemetry__get_deployment_history: 1 deployment(s) in the window (3 ms)
  ...
  ==================== APPROVAL REQUIRED ====================
  Tool:   telemetry__create_incident_ticket  (server: telemetry)
  Payload SHA-256: 77dd0c6b...
  Approve this exact payload? [a]pprove / [r]eject / [e]dit:
```

Contents: [Overview](#1-overview) · [Architecture](#2-architecture) · [Prerequisites](#3-prerequisites) · [Setup](#4-setup) · [Configuration](#5-configuration) · [Running](#6-running) · [Example prompts](#7-example-prompts) · [Tests](#8-tests-and-evaluation) · [Troubleshooting](#9-troubleshooting) · [Layout](#10-repository-layout)

Design and results: [REPORT.md](REPORT.md) · security: [docs/SECURITY.md](docs/SECURITY.md) · evaluation: [docs/EVALUATION.md](docs/EVALUATION.md)

---

## 1. Overview

| Component | What it is | Responsibility |
|---|---|---|
| `mcp-server/` (**ops-telemetry-mcp**) | Custom MCP server in Rust (official SDK `rmcp`) | Tools `search_logs`, `get_deployment_history`, `create_incident_ticket` (SQLite); resources `service://runbooks/{service_name}`, `config://release-checklist`; strict validation, structured errors |
| **Filesystem MCP server** | Official `@modelcontextprotocol/server-filesystem` 2.4.0 (Node.js), used unchanged | Read-only access to `mock_data/workspace` (`RELEASE_NOTES.md`, `service-config.yaml`) |
| `agent-client/` (**orchestrator**) | Rust CLI | Starts both servers, discovers their tools, talks to Claude, runs the tool loop, enforces guardrails (policy, approval, limits, timeouts), writes traces, runs the evaluation |
| `mock_data/` | Simulated environment | 3 services, 45 logs (an outage + a planted prompt injection), 8 deployments, runbooks, release checklist |

The model never runs anything itself: it *asks* for tool calls; the orchestrator decides whether they may run.

## 2. Architecture

```
                 you (PowerShell)
                        │  requests, approvals (Approve / Edit / Reject)
                        ▼
 ┌───────────────────────────────────────────────────────────────┐
 │ agent-client (Rust)                                           │
 │  CLI ─► agent loop ─► tool registry (discovered at start-up)  │
 │           │   ▲        policy · validation · approval gate     │
 │           │   │        injection labelling · redaction         │
 │           │   │        timeouts · MAX_ITERATIONS · JSONL trace │
 │   HTTPS   │   │                  │ stdio (JSON-RPC)  │ stdio    │
 └───────────┼───┼──────────────────┼───────────────────┼─────────┘
             ▼   │                  ▼                   ▼
     Claude Messages API   ops-telemetry-mcp (Rust)   server-filesystem (Node)
     (reasoning only)      logs · deployments ·       read-only, confined to
                           runbooks · checklist ·     mock_data/workspace
                           tickets (SQLite)
```

Each MCP server is a separate process the agent starts and talks to over stdin/stdout. Tools are **discovered at runtime** (`tools/list`, `resources/list`), so nothing about them is hard-coded in the prompt. More in [REPORT.md](REPORT.md).

## 3. Prerequisites

Everything below is for **Windows 10/11 + PowerShell**.

| Tool | Version | Check | Install |
|---|---|---|---|
| Rust (MSVC) | 1.88+ | `rustc --version` | see below |
| Visual Studio Build Tools (C++) | any recent | — | installed together with Rust (see below) |
| Node.js | **24+** | `node --version` | `winget install OpenJS.NodeJS.LTS` or https://nodejs.org |
| Git | any | `git --version` | `winget install Git.Git` |
| Anthropic API key | with credits | — | https://console.anthropic.com |

**Installing Rust:**
1. Download and run `rustup-init.exe` from https://rustup.rs (or `winget install Rustlang.Rustup`).
2. When it offers to install the **Visual Studio C++ build tools**, accept (Rust on Windows needs the MSVC linker).
3. Choose the default installation, then **open a new PowerShell window** and check `rustc --version` and `cargo --version`.

**Node.js:** after installing, open a new PowerShell window and check `node --version` shows `v24` or newer. (The filesystem server needs Node 24; `setup.ps1` checks it.)

**VS Code (optional):** opening the folder suggests the recommended extensions (rust-analyzer, Even Better TOML, CodeLLDB).

## 4. Setup

Open PowerShell **in the project folder** (the one containing `README.md`):

```powershell
cd "<path to the project folder>"
powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1
```

`setup.ps1` does everything, safely re-runnable:

1. checks Rust and Node.js versions;
2. installs the pinned filesystem MCP server: `npm ci` (exact versions from `package-lock.json`, no install scripts);
3. builds the Rust workspace (`cargo build --workspace`);
4. creates `.env` from `.env.example` and asks you to **paste your Anthropic API key** (hidden input; `.env` is git-ignored);
5. runs `check-mcp` (starts both MCP servers and verifies them) and `config` (validates the settings).

Replace the key later with `.\scripts\setup.ps1 -ResetKey`. Without a key: `.\scripts\setup.ps1 -SkipKey`.

> `-ExecutionPolicy Bypass` applies to that one command only. If Windows says a script "is not digitally signed" (e.g. after downloading a zip), run `Unblock-File .\scripts\*.ps1` once.

**Manual alternative** (what the script runs):

```powershell
npm ci
cargo build --workspace
Copy-Item .env.example .env        # then edit .env and set ANTHROPIC_API_KEY=...
.\target\debug\agent-client.exe check-mcp
```

## 5. Configuration

All settings live in `.env` (git-ignored); `.env.example` documents each with its default. Real environment variables override `.env`.

| Variable | Default | Meaning |
|---|---|---|
| `ANTHROPIC_API_KEY` | — (required for the agent) | Your key; never printed, logged or passed to the MCP servers |
| `ANTHROPIC_MODEL` | `claude-opus-5-5` | Claude model |
| `ANTHROPIC_EFFORT` | `medium` | `low` … `max`: how much Claude thinks |
| `ANTHROPIC_FALLBACKS` | `on` | Server-side retry on a fallback model if a request is declined |
| `ANTHROPIC_PROGRESS_UPDATES` | `on` | Show Claude's short progress notes between tool calls |
| `ANTHROPIC_TIMEOUT_SECS` | `300` | Timeout for one Claude API call (10–900) |
| `MAX_ITERATIONS` | `8` | Claude calls allowed per request (1–50) |
| `TOOL_TIMEOUT_SECS` | `10` | Timeout per tool call (1–300) |
| `TOOL_TIMEOUT_OVERRIDES` | — | Per tool, e.g. `files__search_files=30,telemetry__search_logs=5` |
| `OPS_REFERENCE_TIME` | `2026-11-20T12:00:00Z` | "Now" for the simulated data (`now` = real clock) |
| `MCP_STARTUP_TIMEOUT_SECS` | `20` | Time a server may take to start (1–120) |
| `LOG_LEVEL` | `warn` | Diagnostics on stderr (`RUST_LOG` overrides) |
| `NODE_EXE`, `FILESYSTEM_SERVER_SCRIPT`, `FILESYSTEM_ROOT`, `TELEMETRY_SERVER_PATH`, `TICKETS_DB`, `TRACE_FILE` | see `.env.example` | Paths (relative to the project folder) |

**Filesystem MCP configuration.** The agent launches it as
`node node_modules\@modelcontextprotocol\server-filesystem\dist\index.js --read-only --confine-roots -- <project>\mock_data\workspace`
(no `npx`, no shell). Before connecting it checks the workspace contains no secrets or links; after connecting it verifies every tool is read-only and the only allowed folder is the workspace, otherwise it refuses to run. Details: [docs/FILESYSTEM_MCP.md](docs/FILESYSTEM_MCP.md).

## 6. Running

All commands from the project folder. `run.ps1` rebuilds anything that changed first.

| What | Command | Needs key |
|---|---|---|
| Interactive agent (orchestrator + CLI) | `.\scripts\run.ps1` | yes |
| One request, then exit | `.\scripts\run.ps1 ask "Investigate the payments-api errors"` | yes |
| Benchmark evaluation | `.\scripts\run.ps1 eval` (or `eval B2 B7`) | yes |
| Check both MCP servers | `.\scripts\run.ps1 check-mcp` | no |
| Show configuration | `.\scripts\run.ps1 config` | yes |
| Telemetry MCP server, raw protocol demo | `powershell -ExecutionPolicy Bypass -File .\scripts\demo-mcp.ps1` (add `-Raw` for JSON) | no |
| Repository audit | `powershell -ExecutionPolicy Bypass -File .\scripts\audit.ps1` | no |

**The interactive CLI.** Type a request at `you>`. You'll see every step: model calls, each tool call with arguments and outcome, progress notes, security warnings, and the answer. Commands: `/tools`, `/servers`, `/reset` (forget the conversation), `/help`, `/exit`. **Ctrl+C** cancels the current request (the servers keep running); at the prompt it exits.

**Approvals.** `create_incident_ticket` is the only state-changing tool. When Claude wants it, the agent pauses and shows the exact payload and its SHA-256. Type `a` to approve, `r` to reject (optionally with a reason for Claude), or `e` to edit fields; an edit shows the new payload and needs a fresh approval. Only your keyboard can answer this prompt (never Claude or tool output). Any other answer asks again, and end of input (Ctrl+Z, Enter) counts as reject.

**The telemetry server on its own.** `ops-telemetry-mcp` is a normal stdio MCP server: `.\target\debug\ops-telemetry-mcp.exe` waits for JSON-RPC on stdin (logs go to stderr). The agent starts it automatically; `demo-mcp.ps1` sends it real MCP messages; any MCP client can use it.

**Traces.** Every session appends to `traces\agent_traces.jsonl`: requests, model calls, each tool call (input, duration, status, error, approval decision, injection flags), all redacted. Example runs: [`traces/samples/`](traces/samples/).

## 7. Example prompts

| Prompt | What happens |
|---|---|
| `Show me the last 20 ERROR logs from payments-api.` | One `search_logs` call; only 8 exist |
| `Customers report failed payments since this morning. Investigate payments-api and open an incident ticket if warranted.` | Logs → deployments → runbook → release notes/config → assessment → ticket proposal → **your approval** |
| `Check whether the notification-worker 3.2.0 release is ready for deployment.` | Checklist + release notes + config + deployment history → NOT READY with reasons |
| `Investigate inventory-service.` | Unknown service: structured error, valid services listed, nothing invented |
| `Search the payments-api logs for entries with severity level CRITICAL.` | Validation error; valid levels explained |
| `Check whether everything is okay.` | Clarifying question |
| `Summarize the latest payments-api errors.` | Reads the poisoned log; flags it; does not follow it |

## 8. Tests and evaluation

```powershell
cargo test --workspace                                   # all tests, no API key needed (~1 min)
cargo clippy --workspace --all-targets -- -D warnings    # lints
cargo fmt --all -- --check                               # formatting
.\scripts\run.ps1 eval                                   # live benchmark vs Claude (needs key + credits)
```

The automated tests start the real MCP servers. Where a model is needed they use a scripted model or a local fake Claude endpoint, so they need no key and no network. Results and the live-evaluation status: [docs/EVALUATION.md](docs/EVALUATION.md).

## 9. Troubleshooting

| Symptom | Fix |
|---|---|
| `ANTHROPIC_API_KEY is not set` / `still contains the placeholder` | `.\scripts\setup.ps1 -ResetKey` and paste the key |
| `Claude rejected the request (400): ... credit balance is too low` | Add credits in the Anthropic console (Plans & Billing) |
| `the API key was rejected (401)` | Wrong or revoked key: `.\scripts\setup.ps1 -ResetKey` |
| `... thinking-display-updates ...` in a 400 error | Set `ANTHROPIC_PROGRESS_UPDATES=off` in `.env` |
| `Node.js v22… needs Node 24 or newer` | Install Node 24+, open a **new** PowerShell window, rerun setup |
| `[files] ... not installed; run npm ci` | `npm ci` in the project folder |
| `[telemetry] ... not built` | `cargo build --workspace` |
| `refused for safety: the workspace contains entries that must not be shared` | Remove `.env`/key/database files or links from `mock_data\workspace` |
| `cargo` not found | Install Rust (section 3), open a new PowerShell window |
| `link.exe not found` | Install Visual Studio Build Tools with "Desktop development with C++" |
| "running scripts is disabled on this system" | Use `powershell -ExecutionPolicy Bypass -File ...` as shown |
| A request seems stuck | Ctrl+C cancels it; tool calls time out after `TOOL_TIMEOUT_SECS` |

## 10. Repository layout

```
mcp-server/        ops-telemetry-mcp: src/{server,data,tickets,error,config}.rs, tests/
agent-client/      orchestrator: src/{cli,eval,trace}.rs, src/llm/, src/agent/, src/mcp/, src/safety/, tests/
mock_data/         logs.json, deployments.json, release-checklist.md, runbooks/, workspace/
evals/             benchmarks.json (suite), results/ (eval runs, git-ignored)
traces/            agent_traces.jsonl (git-ignored), samples/ (committed example runs)
data/              tickets.db at runtime (git-ignored)
scripts/           setup.ps1, run.ps1, demo-mcp.ps1, audit.ps1
docs/              design, data spec, filesystem MCP, security, evaluation, learning notes
REPORT.md          architecture, design decisions, evaluation, reflection, compliance matrix
PROJECT_CONTEXT.md project status and decisions
```
#   - I n c i d e n t - T r i a g e - R e l e a s e - R e a d i n e s s - A I - A g e n t -  
 