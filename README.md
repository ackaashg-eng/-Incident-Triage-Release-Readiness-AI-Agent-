# Incident Triage & Release Readiness Agent

A learning project that investigates simulated service incidents and checks whether a release is ready. The working command-line agent is written in Rust. It uses an LLM for reasoning and MCP servers to read sample logs, deployments, runbooks, and release files.

> **This project uses mock data.** It does not connect to real production services. The desktop UI is an early prototype; its agent responses and server status are simulated. Use the CLI for the functional agent workflow.

## What it does

You ask a question such as “Investigate the payments-api errors.” The agent can:

- Search sample logs and deployment history.
- Read service runbooks, release notes, configuration, and a release checklist.
- Summarize evidence and recommend next steps.
- Propose an incident ticket. The ticket is saved only after you review and approve it.

The model proposes tool calls; the Rust agent validates and runs them. It blocks unknown or invalid tool requests, limits model/tool wait times, and labels suspicious text returned from tools as untrusted data.

## How the pieces fit together

```text
You (PowerShell)
      |
      v
agent-client (Rust CLI) <----HTTPS----> Anthropic or OpenRouter
      |
      +----stdio / MCP----> ops-telemetry-mcp (Rust)
      |                       logs, deployments, runbooks, tickets
      |
      +----stdio / MCP----> filesystem MCP server (Node.js)
                              read-only mock_data/workspace/
```

`agent-client` starts both MCP servers and discovers their tools at startup. The telemetry server reads the simulated operations data. The filesystem server is restricted to the two sample files in `mock_data/workspace/`.

## Repository map

| Folder or file | Purpose |
|---|---|
| `agent-client/` | Rust CLI, model API clients, agent loop, MCP connections, validation, approvals, safety, tracing, and tests. |
| `mcp-server/` | Rust telemetry MCP server and protocol/data tests. |
| `desktop-ui/` | Early Tauri + React interface prototype. It currently uses hard-coded/demo responses; it is not connected to the real CLI agent. |
| `mock_data/` | Sample logs, deployments, runbooks, release checklist, release notes, and service configuration. |
| `scripts/` | PowerShell setup, run, MCP demo, and audit scripts. |
| `docs/` | Security, design, data, evaluation, learning notes, and the beginner project guide. |
| `evals/benchmarks.json` | Seven repeatable benchmark scenarios for the CLI agent. |
| `traces/samples/` | Example JSONL traces and matching console output. The model replies in these examples were scripted, not live Claude responses. |
| `data/` | Runtime SQLite ticket database location; database files are ignored by Git. |
| `Cargo.toml` | Root Rust workspace containing `agent-client` and `mcp-server`. |

For beginner explanations of the Rust CLI and telemetry-server code, see [docs/PROJECT_GUIDE.md](docs/PROJECT_GUIDE.md). For the current desktop prototype status, use this README. For design and recorded results, see [REPORT.md](REPORT.md) and [docs/EVALUATION.md](docs/EVALUATION.md).

## Requirements

The supported setup documented here is Windows 10/11 with PowerShell:

- Rust 1.88 or newer, with the MSVC toolchain.
- Visual Studio C++ Build Tools (required by Rust on Windows).
- Node.js 24 or newer.
- Git.
- An API key for the model provider you choose. The agent supports Anthropic and OpenRouter.

Check installed tools with:

```powershell
rustc --version
cargo --version
node --version
git --version
```

## First-time setup

Open PowerShell in the project root (the folder containing this `README.md`), then run:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1
```

The script checks Rust and Node, installs the pinned filesystem MCP package, builds the Rust workspace, creates `.env` from `.env.example` if needed, prompts for an Anthropic API key, and checks the MCP servers and configuration.

The setup script's key prompt is for Anthropic. To use OpenRouter instead, run setup with `-SkipKey`, set the OpenRouter values in `.env` as described under [Choosing a model provider](#choosing-a-model-provider), and then check the settings with `./scripts/run.ps1 config`.

To run setup without entering a key:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1 -SkipKey
```

The `.env` file is local and ignored by Git. Never commit a real API key.

## Choosing a model provider

Anthropic is the default provider. The starter `.env.example` shows the Anthropic settings. The active provider is selected with `LLM_PROVIDER`; process environment variables override values in `.env`.

### Anthropic

Set a real key in `.env`:

```dotenv
ANTHROPIC_API_KEY=your-key-here
ANTHROPIC_MODEL=claude-opus-5-5
```

Other supported settings include `ANTHROPIC_EFFORT`, `ANTHROPIC_FALLBACKS`, `ANTHROPIC_PROGRESS_UPDATES`, `ANTHROPIC_TIMEOUT_SECS`, and `ANTHROPIC_BASE_URL`.

### OpenRouter

Add these settings to `.env` and use your own OpenRouter key and model slug:

```dotenv
LLM_PROVIDER=openrouter
OPENROUTER_API_KEY=your-key-here
OPENROUTER_MODEL=provider/model-name
# Optional; this is the default endpoint
OPENROUTER_BASE_URL=https://openrouter.ai/api/v1
# Optional comma-separated backup model slugs
# OPENROUTER_FALLBACKS=provider/backup-model
```

Do not include the angle-bracket examples literally; replace them with your own values. `agent-client config` shows the selected provider and model, but masks the key.

## Run the agent

Run all commands from the project root.

### Interactive mode

```powershell
.\scripts\run.ps1
```

Type a question at the `you>` prompt. The session stays open for follow-up questions. Available commands are `/tools`, `/servers`, `/reset`, `/help`, and `/exit`.

### One question, then exit

```powershell
.\scripts\run.ps1 ask "Investigate the payments-api errors"
```

`ask` runs one request and exits after the answer. If the model ends its answer with a follow-up question, that does not keep the session open; start interactive mode for a conversation.

### Other useful commands

```powershell
# Display the current provider, model, paths, and masked key
.\scripts\run.ps1 config

# Start and check the MCP servers; no model API key is needed
.\scripts\run.ps1 check-mcp

# Run all benchmark cases (requires an API key and provider access)
.\scripts\run.ps1 eval

# Run selected benchmark cases
.\scripts\run.ps1 eval B2 B7

# Exercise the telemetry MCP server without calling a model
powershell -ExecutionPolicy Bypass -File .\scripts\demo-mcp.ps1

# Run the repository audit
powershell -ExecutionPolicy Bypass -File .\scripts\audit.ps1
```

## Ticket approvals

`create_incident_ticket` is the only tool that changes stored data. If the agent proposes a ticket, the CLI pauses and displays the exact fields and a SHA-256 fingerprint:

- Type `a` to approve that exact ticket.
- Type `r` to reject it.
- Type `e` to edit it. The edited ticket is shown again and needs a new approval.

The model cannot answer the approval prompt for you. If you reject the ticket, it is not saved. A final sentence such as “Shall I open a ticket?” is only a question; a ticket is created only when the tool is called and you approve the displayed proposal.

## Configuration reference

The main settings are:

| Setting | Purpose |
|---|---|
| `LLM_PROVIDER` | `anthropic` (default) or `openrouter`. |
| `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY` | Provider credentials. Set the key for the selected provider only. |
| `ANTHROPIC_MODEL` | Anthropic model name; defaults to `claude-opus-5-5`. |
| `OPENROUTER_MODEL` | Required OpenRouter model slug, for example `provider/model-name`. |
| `MAX_ITERATIONS` | Maximum model calls for one request; default 8. |
| `TOOL_TIMEOUT_SECS` | Default timeout per tool call; default 10 seconds. |
| `TOOL_TIMEOUT_OVERRIDES` | Optional per-tool timeouts, such as `telemetry__search_logs=5`. |
| `ANTHROPIC_TIMEOUT_SECS` | Model request timeout; default 300 seconds. |
| `OPS_REFERENCE_TIME` | Reference “now” for the fixed sample data; default `2026-11-20T12:00:00Z`. |
| `MCP_STARTUP_TIMEOUT_SECS` | MCP server startup/handshake timeout. |
| `FILESYSTEM_ROOT` | Filesystem server's allowed directory; default `mock_data/workspace`. |
| `TICKETS_DB` | SQLite ticket database path; default `data/tickets.db`. |
| `TRACE_FILE` | Agent JSONL trace path; default `traces/agent_traces.jsonl`. |
| `LOG_LEVEL` | Diagnostic log level; messages go to stderr. |

See [.env.example](.env.example) and `agent-client/src/config.rs` for supported names, defaults, and validation. The actual `.env` is not committed.

## Mock scenarios

The sample dataset contains three services: `payments-api`, `auth-service`, and `notification-worker`. It includes:

- A payment outage correlated with the `payments-api` 2.14.0 deployment.
- An `auth-service` key-rotation issue that is not caused by its most recent deployment.
- A `notification-worker` release candidate that is not ready because several checklist items fail.
- One log entry containing a customer-supplied prompt-injection attempt, used to demonstrate untrusted-data handling.

These are teaching scenarios, not live production observations. `docs/MOCK_DATA_SPEC.md` contains the detailed data rules and expected findings.

## Security behavior

- Tool names are discovered from MCP servers, then checked against an application-owned allowlist.
- Tool arguments are validated before the server receives them. For example, a numeric limit must be sent as a number; the string `"50"` is rejected.
- The filesystem server is launched read-only and confined to `mock_data/workspace/`.
- Ticket creation requires human approval, and approval is tied to the exact payload.
- Tool output is treated as untrusted. Known suspicious phrases are flagged, and sensitive strings/paths are redacted from traces.
- Model calls and tool calls have configurable iteration and timeout limits.

These controls reduce risk; the phrase-based prompt-injection scanner does not detect every possible attack. More details are in [docs/SECURITY.md](docs/SECURITY.md) and [docs/FILESYSTEM_MCP.md](docs/FILESYSTEM_MCP.md).

## Desktop UI status

`desktop-ui/` contains a Tauri v2 and React interface prototype. Its current Rust commands return hard-coded server status and a scripted sample investigation; the browser preview also uses demo fallbacks. It is not yet wired to the real `agent-client` loop, live provider, or CLI approval gate. Treat it as a UI prototype, not as an alternate working agent.

To preview the frontend:

```powershell
cd desktop-ui
npm ci
npm run dev
```

To launch the Tauri development shell:

```powershell
cd desktop-ui
npm ci
npm run tauri dev
```

To build the desktop app:

```powershell
cd desktop-ui
npm run tauri build
```

The Tauri build needs the Rust MSVC toolchain and Windows desktop prerequisites. Build output is generated locally and should not be committed.

## Tests and checks

The Rust workspace tests use mock data and scripted model responses; they do not require a live provider key:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The live benchmark command `./scripts/run.ps1 eval` calls the selected model provider and may incur provider charges. Functional checks do not establish the quality of live model reasoning. See [docs/EVALUATION.md](docs/EVALUATION.md) for the results recorded by the project.

## Troubleshooting

| Problem | What to check |
|---|---|
| `opencode` is not recognized | OpenCode is a separate tool and is not required. Use `./scripts/run.ps1` for this project's interactive CLI. |
| Missing/placeholder Anthropic key | Set `ANTHROPIC_API_KEY` in `.env`, or choose OpenRouter and set `LLM_PROVIDER`, `OPENROUTER_API_KEY`, and `OPENROUTER_MODEL`. |
| OpenRouter says the model is missing | Set `OPENROUTER_MODEL` to a valid model slug from your OpenRouter account/catalog. |
| Tool says `expected integer ... got string` | The model sent a number as quoted text, such as `"50"`. The request was blocked before the tool ran. Retry, ask without a limit, or try a model that follows the tool's JSON schema more reliably. |
| MCP server/package is missing | Run setup again, or run `npm ci` at the project root and `cargo build --workspace`. |
| Node.js version is too old | Install Node.js 24 or newer, open a new PowerShell window, and rerun setup. |
| Rust linker `link.exe` is missing | Install Visual Studio Build Tools with the “Desktop development with C++” workload. |
| PowerShell blocks a script | Run it with `powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1` as shown above. |
| Agent stops after one answer | `ask` is one-shot. Run `./scripts/run.ps1` with no arguments for an interactive session. |

## More documentation

- [Beginner project guide](docs/PROJECT_GUIDE.md) — code and file explanations.
- [Technical report](REPORT.md) — architecture, design decisions, evidence, and limitations.
- [Security notes](docs/SECURITY.md) — trust boundaries and safeguards.
- [Evaluation report](docs/EVALUATION.md) — benchmark design and recorded results.
- [Filesystem MCP notes](docs/FILESYSTEM_MCP.md) — how the read-only file server is confined.
- [Mock data specification](docs/MOCK_DATA_SPEC.md) — schemas and scenario ground truth.
