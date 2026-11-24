# Incident Triage & Release Readiness AI Agent (Rust + MCP)

A command-line AI agent that investigates simulated production incidents and checks release readiness. It uses **Claude** for reasoning and the **Model Context Protocol (MCP)** to reach its tools:

* **`ops-telemetry-mcp`**: a custom Rust MCP server for logs, deployment history, runbooks, the release checklist and incident tickets.
* **`@modelcontextprotocol/server-filesystem`**: the official MCP server, run read-only on `mock_data/workspace` (release notes, service config).
* **`agent-client`**: the Rust orchestrator. It runs the agent loop, enforces guardrails, asks a human before any ticket is created, and writes a trace of every tool call.

> **Status:** Phase 1 (project foundation). The agent loop, tools and evaluation arrive in later phases; see `PROJECT_CONTEXT.md`.
> Full design: [`docs/PHASE_0_DESIGN.md`](docs/PHASE_0_DESIGN.md).

---

## Prerequisites (Windows)

| Tool | Version | Check in PowerShell | Get it |
|---|---|---|---|
| Rust (MSVC toolchain) | 1.88 or newer | `rustc --version` | https://rustup.rs |
| Visual Studio Build Tools | C++ workload | installed with rustup's prompt | https://visualstudio.microsoft.com/downloads/ |
| Node.js | **24 or newer** | `node --version` | https://nodejs.org (LTS installer) or `winget install OpenJS.NodeJS.LTS` |
| Git | any recent | `git --version` | https://git-scm.com |
| Anthropic API key | — | — | https://console.anthropic.com |

VS Code: when you open the folder, accept the prompt to install the recommended extensions (rust-analyzer, Even Better TOML, CodeLLDB).

---

## Setup (one time)

Run these in PowerShell **from the project folder**:

```powershell
cd "C:\Users\ASUS\Music\SpilLabs\Agent MCP Ochestra"
powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1
```

The script:

1. checks Rust and Node.js versions;
2. installs the pinned filesystem MCP server (`npm ci`, no install scripts);
3. builds the Rust workspace;
4. creates `.env` from `.env.example` and asks you to **paste your API key** (the input is hidden);
5. runs a foundation check.

It is safe to run again. To replace the key: `.\scripts\setup.ps1 -ResetKey`.

> `-ExecutionPolicy Bypass` applies to that one command only. If Windows says a script "is not digitally signed" (e.g. after downloading a zip), run `Unblock-File .\scripts\*.ps1` once.

### Configuration

All settings live in `.env` (git-ignored). `.env.example` documents each one with its default. Real environment variables override `.env`.

| Variable | Default | Meaning |
|---|---|---|
| `ANTHROPIC_API_KEY` | — (required) | Your key; never printed or logged |
| `ANTHROPIC_MODEL` | `claude-opus-5-5` | Claude model |
| `ANTHROPIC_EFFORT` | `medium` | `low` … `max` |
| `ANTHROPIC_FALLBACKS` | `on` | Server-side retry on a fallback model if a request is declined |
| `ANTHROPIC_PROGRESS_UPDATES` | `on` | Show Claude's short progress notes between tool calls |
| `ANTHROPIC_TIMEOUT_SECS` | `300` | Timeout for one Claude API call (10–900) |
| `MAX_ITERATIONS` | `8` | Claude calls allowed per user request (1–50) |
| `TOOL_TIMEOUT_SECS` | `10` | Per-tool timeout (1–300) |
| `OPS_REFERENCE_TIME` | `2026-11-20T12:00:00Z` | "Now" for the simulated data (`now` = real clock) |
| `LOG_LEVEL` | `warn` | Diagnostics on stderr (`RUST_LOG` overrides) |
| `TOOL_TIMEOUT_OVERRIDES` | — | Per-tool timeouts, e.g. `files__search_files=30,telemetry__search_logs=5` |
| `MCP_STARTUP_TIMEOUT_SECS` | `20` | Time a server may take to start (1–120) |
| `NODE_EXE` | `node` | Node.js executable for the filesystem server |

---

## Run

```powershell
.\scripts\run.ps1                                              # interactive session
.\scripts\run.ps1 ask "Investigate the payments-api errors"     # one request, then exit
.\scripts\run.ps1 eval                                         # run the benchmark suite against Claude
.\scripts\run.ps1 check-mcp                                    # test both MCP servers (no key needed)
.\scripts\run.ps1 config                                       # show the loaded configuration
```

The script builds anything that changed and starts the agent from the project folder. The agent starts both MCP servers, discovers their tools, and shows every step while it works:

```
you> Investigate the payment API errors
  [step 1/8] asking Claude...
  ~ Checking recent payments-api errors and deployments first.
  -> telemetry__search_logs {"service_name":"payments-api","severity":"ERROR","limit":10}
     ok telemetry__search_logs: 8 of 8 matching log entries (12 ms)
  -> telemetry__get_deployment_history {"service_name":"payments-api","window_hours":24}
     ok telemetry__get_deployment_history: 1 deployment(s) in the window (4 ms)
  [step 2/8] asking Claude...
  ...
claude> Summary: ...
```

Interactive commands: `/tools`, `/servers`, `/reset` (forget the conversation), `/help`, `/exit`. Ctrl+C cancels the current request; at the prompt it exits.

**Safety:** a tool that changes something (`create_incident_ticket`) pauses the agent and shows you the exact payload and its SHA-256:

```text
  ==================== APPROVAL REQUIRED ====================
  Tool:   telemetry__create_incident_ticket  (server: telemetry)
  | { "assignee": "payments-oncall", "severity": "CRITICAL", ... }
  Payload SHA-256: 3f9a...
  Approve this exact payload? [a]pprove / [r]eject / [e]dit:
```

Only what you approve runs; an edit needs a fresh approval. Tool output is labelled as untrusted data, instruction-like text is flagged, secrets are redacted, and every step is written to `traces\agent_traces.jsonl`. Details and test evidence: [docs/SECURITY.md](docs/SECURITY.md).

---

## The telemetry MCP server (`ops-telemetry-mcp`)

A standalone MCP server (stdio transport, official Rust SDK `rmcp`). Any MCP client can use it; the agent is just one client.

| Kind | Name | What it does |
|---|---|---|
| Tool (read-only) | `search_logs(service_name, severity?, limit?)` | Logs of one service, newest first; severity INFO/WARN/ERROR/FATAL (omit = all); limit 1–100 (default 20) |
| Tool (read-only) | `get_deployment_history(service_name, window_hours)` | Deployments/rollbacks in the last 1–720 h: version, commit, timestamp, rollback status |
| Tool (**write**) | `create_incident_ticket(title, severity, summary, assignee)` | Validates and stores a ticket in SQLite (`data\tickets.db`); returns `INC-NNNN` + timestamp |
| Resource | `service://runbooks/{service_name}` | Runbook: troubleshooting steps and escalation rules |
| Resource | `config://release-checklist` | Mandatory release criteria RC-1…RC-9 |

Valid services: `auth-service`, `payments-api`, `notification-worker`. Errors come back as `{"error": {"code", "message", "hint", "valid_values", "field_errors"}}`; the server never crashes on a bad request.

See it speak raw MCP (no API key needed):

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\demo-mcp.ps1        # one summary line per response
powershell -ExecutionPolicy Bypass -File .\scripts\demo-mcp.ps1 -Raw   # the exact JSON-RPC messages
```

The demo writes tickets to `data\demo-tickets.db`, not the real store.

---

## The filesystem MCP server (external)

The official `@modelcontextprotocol/server-filesystem` 2.4.0 (installed by `npm ci`) runs unchanged as a separate Node process, **read-only** and confined to `mock_data\workspace` (which holds only `RELEASE_NOTES.md` and `service-config.yaml`). It is launched with a scrubbed environment, so it never sees your API key. Details and test documentation: [docs/FILESYSTEM_MCP.md](docs/FILESYSTEM_MCP.md).

Check both MCP servers end to end (starts them, discovers tools, reads the files, tries to escape the workspace, shuts down; no API key needed):

```powershell
.\scripts\run.ps1 check-mcp
```

---

## Development commands

Run from the project folder:

```powershell
cargo build --workspace                              # compile both crates
cargo test --workspace                               # all tests (no API key needed)
cargo clippy --workspace --all-targets -- -D warnings  # lints, warnings = errors
cargo fmt --all                                      # format the code
```

---

## Project layout

```
mcp-server/     ops-telemetry-mcp (custom MCP server, Rust)
agent-client/   the orchestrator CLI (Rust)
mock_data/      simulated logs, deployments, runbooks, workspace files   (Phase 2)
data/           runtime SQLite ticket store            (git-ignored contents)
traces/         agent_traces.jsonl (git-ignored) + samples/ for submission
evals/          benchmarks.json (suite) + results/ (eval runs, git-ignored)
scripts/        setup.ps1, run.ps1, demo-mcp.ps1
docs/           design + per-phase learning notes
```

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| `ANTHROPIC_API_KEY is not set` / `still contains the placeholder` | Run `.\scripts\setup.ps1` and paste the key |
| `Node.js v22… needs Node 24 or newer` | Install Node.js 24+ LTS, open a **new** PowerShell window, rerun setup |
| `cargo` not found | Install Rust via rustup, then open a new PowerShell window |
| `link.exe not found` during build | Install Visual Studio Build Tools with "Desktop development with C++" |
| Script "cannot be loaded because running scripts is disabled" | Use `powershell -ExecutionPolicy Bypass -File …` as shown above |
