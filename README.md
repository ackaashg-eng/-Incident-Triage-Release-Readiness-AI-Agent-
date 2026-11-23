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
| `MAX_ITERATIONS` | `8` | Claude calls allowed per user request (1–50) |
| `TOOL_TIMEOUT_SECS` | `10` | Per-tool timeout (1–300) |
| `OPS_REFERENCE_TIME` | `2026-11-20T12:00:00Z` | "Now" for the simulated data (`now` = real clock) |
| `LOG_LEVEL` | `warn` | Diagnostics on stderr (`RUST_LOG` overrides) |

---

## Run

```powershell
.\scripts\run.ps1
```

This builds anything that changed and starts the agent from the project folder. In Phase 1 it prints a foundation check (model, limits, paths, `API key: loaded (***)`).

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
evals/          benchmark prompts and results           (Phase 7)
scripts/        setup.ps1, run.ps1
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
