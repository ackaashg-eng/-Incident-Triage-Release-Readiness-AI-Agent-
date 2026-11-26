# Beginner's Guide to the Incident Triage and Release Readiness Agent

**Project:** Incident Triage & Release Readiness AI Agent (Rust + MCP)  
**Audience:** Readers new to this project, Rust, or MCP  
**Prepared:** 25 November 2026

## 1. The project in one paragraph

This project is a command-line assistant for investigating simulated production incidents and checking whether a software release is ready. The Rust program called `agent-client` sends a question and a discovered list of tools to Anthropic's Claude Messages API. Claude may ask to search simulated logs, inspect deployment history, read runbooks or release files, or propose an incident ticket. The Rust program—not Claude—starts the tool servers, checks each request, runs permitted tools, and returns their results. Creating a ticket changes SQLite data, so a person must approve the exact ticket first.

The operations information is sample data. This repository does not connect to a real production system.

## 2. How a request moves through the project

1. You run the CLI and type a question, such as “Investigate payments-api.”
2. `agent-client` reads configuration, starts the Rust telemetry server and the Node.js filesystem server, and asks them what tools and resources they offer.
3. It sends the conversation and the discovered tool descriptions to Claude over HTTPS.
4. Claude returns either an answer or a structured request to use a tool.
5. Before a tool runs, the Rust agent checks that it exists, that policy permits it, and that its arguments match the server's schema. Tool calls also have time limits.
6. The MCP server reads the mock logs, deployments, runbooks, checklist, or workspace files and returns data. The agent labels suspicious instruction-like text as untrusted and redacts sensitive strings before writing traces.
7. The agent sends the result back to Claude. It repeats this loop until Claude answers, an error occurs, or the configured call limit is reached.
8. If Claude proposes `create_incident_ticket`, the CLI shows the proposed fields and a SHA-256 fingerprint. A person can approve, edit, or reject. An edit requires a new approval of the edited values.

The usual maximum is eight Claude calls for one request. The maximum is configurable.

```text
You -> agent-client -> Claude
             |            |
             | tool request|
             v            |
      MCP servers --------+
       |            |
   telemetry     read-only files
   logs/tickets  workspace files
```

## 3. The two Rust programs

| Program | Folder | Beginner description |
|---|---|---|
| `agent-client` | `agent-client/` | The coordinator and command-line interface. It talks to Claude and the MCP servers, enforces rules, and records traces and evaluations. |
| `ops-telemetry-mcp` | `mcp-server/` | The custom tool server. It reads the simulated operations data and stores approved incident tickets in SQLite. |

There is also an external Node.js filesystem MCP server. The agent starts it in read-only mode and confines it to `mock_data/workspace/`. The Node package is installed from the pinned dependency in `package-lock.json`; it is not application source written in this repository.

## 4. The main tools and resources

The telemetry server offers three tools:

| Tool | What it does | Changes data? |
|---|---|---|
| `search_logs` | Finds log entries by service, severity, and result limit. | No |
| `get_deployment_history` | Shows deployment and rollback history for a service and time window. | No |
| `create_incident_ticket` | Validates and saves a new ticket in SQLite. | Yes; requires human approval in the agent |

The telemetry server also offers read-only resources: `service://runbooks/{service_name}` and `config://release-checklist`. The filesystem server offers read-only file-reading tools for release notes and service configuration. A **tool** is an operation that can be called; a **resource** is read-only information addressed by a URI.

## 5. Beginner glossary: project and programming words

| Word | Plain-language meaning here |
|---|---|
| **Agent / agent loop** | A program that can ask a model what to do, use tools, inspect the results, and ask again. The loop is “think, act, observe, repeat.” |
| **LLM / Claude** | A language model that interprets the question and writes an answer or structured tool request. It does not itself run the tools. |
| **Orchestrator** | The program that coordinates the model, tools, policy checks, approvals, and results. Here, that is `agent-client`. |
| **MCP** | Model Context Protocol: a shared format for programs to discover and call tools and read resources. It is similar to a standard connector. |
| **MCP client / server** | A client asks for capabilities and invokes them; a server offers the tools or resources. `agent-client` is an MCP client; `ops-telemetry-mcp` is a server. |
| **stdio** | Standard input and output streams of a process. This project uses them to send MCP messages between local processes. Protocol messages use stdout; diagnostics use stderr. |
| **JSON / JSON Schema** | JSON is a common format for structured data. A JSON Schema describes what fields and types are allowed. |
| **Crate / Cargo / workspace** | A crate is a Rust package. Cargo builds and manages Rust packages. A workspace groups this project's two crates under one root `Cargo.toml` and lockfile. |
| **Dependency** | A library or external package this project uses, such as Tokio, Serde, or the MCP SDK. |
| **Async / Tokio** | Asynchronous code can wait for network, process, or timer work without blocking the whole program. Tokio runs that work. |
| **Trait** | A named interface describing behavior a type can provide. For example, the agent can use different model clients or approval implementations through shared interfaces. |
| **Serialization / deserialization** | Turning in-memory data into a format such as JSON, or reading that format back into program data. Serde supports this in Rust. |
| **SQLite** | A small database stored in a file. The project uses it to persist ticket records. |
| **Timeout** | A limit on how long one operation may wait before it is reported as failed. |
| **Prompt injection** | Instruction-like text hidden inside data (for example, a log message) that attempts to mislead a model. The project treats tool output as untrusted and applies access and approval limits in code. |
| **Redaction** | Replacing secret-looking values or private paths with safe placeholders before they are displayed or recorded. |
| **SHA-256** | A one-way fingerprint function. Before ticket approval, it helps show whether the exact proposed payload has changed. |
| **JSONL** | JSON Lines: a file containing one independent JSON object per line. The trace writer appends events in this format. |
| **Benchmark / evaluation** | A repeatable question with expected tool use or expected facts, used to measure behavior. A live model evaluation is separate from deterministic tests. |
| **Fixture / fake / scripted model** | Test-only substitute data or service. It lets tests check behavior without using a real API key or calling a live model. |
| **`pub` / private** | Rust's `pub` keyword makes an item accessible outside its current module. Items without `pub` are private to their module. |
| **`Result<T, E>` / `?`** | `Result` represents either a successful value `T` or an error `E`. `?` returns an error early to the caller. |
| **`Option<T>`** | A value that may be present (`Some`) or absent (`None`). |
| **`struct` / `enum`** | A `struct` groups named fields; an `enum` represents one of a fixed set of variants. |
| **`impl`** | A block that defines methods for a type. |
| **`async fn` / `.await`** | An asynchronous function; `.await` pauses that task until its result is ready. |
| **`#[derive(...)]` / `#[serde(...)]`** | Attributes that tell the compiler or a library to generate or adjust behavior, often for JSON conversion. |
| **`#[cfg(test)]`** | Marks test-only code so it is compiled for tests, not normal application runs. |

## 6. Code reading guide: `agent-client`

The files below are listed individually. “Key functions” highlights the main runtime entry points and useful helpers; test-only helper functions are described with their test files later in this guide.

### 6.1 CLI, configuration, tracing, and evaluation

| File | What it is for | Key functions and types |
|---|---|---|
| `agent-client/Cargo.toml` | Manifest for the Rust coordinator crate: crate name, dependencies, and shared workspace settings. | Not a Rust source file. See the glossary for crate and dependency. |
| `agent-client/src/main.rs` | Starts the executable, reads the command-line mode, loads configuration, and runs commands such as `ask`, `eval`, `config`, and `check-mcp`. | `main`, `run`, `eval_options`, `load`, `runtime`, `print_config`. |
| `agent-client/src/lib.rs` | Declares the library modules so application code and tests can use the same implementation. | Module declarations; no user-facing runtime function. |
| `agent-client/src/cli.rs` | Connects the command-line interface to the agent. Prints progress and tool activity, supports interactive sessions, and handles one-shot questions. | `ConsoleObserver::on_event`, `agent_settings`, `Session::run`, `Session::close`, `start_session`, `interactive`, `ask`; printing helpers include `print_tools` and `print_report`. |
| `agent-client/src/config.rs` | Reads `.env` and environment variables, checks allowed values and ranges, protects the API key from formatting, and resolves file paths. | `ApiKey::new` / `expose`, `Config::load` / `from_lookup`, `McpSettings::load` / `from_lookup`, `load_dotenv`, `log_level`, `parse_timeout_overrides`, `parse_on_off`, `parse_number`, `resolve`. |
| `agent-client/src/logging.rs` | Sets up diagnostic logging. Logs go to stderr so they do not corrupt stdio MCP messages. | `init`. |
| `agent-client/src/check.rs` | Implements `check-mcp`, which starts both servers and checks discovery, read access, and denied access. | `run`, `check_filesystem`, `check_telemetry`, `read_text`, `expect_denied`, `text_of`, `file_uri`. |
| `agent-client/src/trace.rs` | Appends session and agent events to JSONL; includes positions, results, approval status, and redaction. | `Tracer::open`, `record`, `session_start`, `session_end`, `new_session_id`; `Observer::on_event` records agent events. |
| `agent-client/src/eval.rs` | Loads the benchmark suite, checks model/tool outcomes against expectations, records verdicts, and renders a report. | `load_suite`, `evaluate`, `run`, `render_report`; `Checks::matches` / `describe`; result types include `Benchmark`, `Verdict`, `CheckResult`, and `BenchmarkResult`. |

### 6.2 The agent and its guardrails

| File | What it is for | Key functions and types |
|---|---|---|
| `agent-client/src/agent/mod.rs` | Groups the agent's internal modules. | Declares `approval`, `prompt`, `registry`, `runner`, and `validate`. |
| `agent-client/src/agent/runner.rs` | Main agent loop. Sends requests to the model, handles tool calls, enforces iteration/time limits, returns tool output, and builds a turn report. | `Agent::new`, `run_turn`, `reset`, `recover_interrupted`, `shutdown`; internal flow includes `execute`, `run_tool`, `run_with_approval`, `call_server`; `render_call_result` and `summarise` format results. Important types: `AgentSettings`, `ToolStatus`, `ApprovalStatus`, `AgentEvent`, `TurnOutcome`, `TurnReport`. |
| `agent-client/src/agent/registry.rs` | Converts server-discovered tools/resources into namespaced model tools and assigns a trusted permission policy. | `ToolRegistry::build`, `get`, `iter`, `claude_tools`, `classify`, `adapt_schema`, `resource_uri_allowed`. Important types: `Permission`, `ToolTarget`, `RegisteredTool`, `DiscoveredServer`. |
| `agent-client/src/agent/validate.rs` | Checks model-supplied JSON arguments against tool schemas before sending them to a server. | `validate`; recursive helpers `check`, `has_type`, and `type_name`. |
| `agent-client/src/agent/prompt.rs` | Builds the system instructions sent to Claude, including how to use tools and treat tool results. | `system_prompt`; `PromptContext` supplies the dynamic server context. |
| `agent-client/src/agent/approval.rs` | Defines approval requests and decisions, console interaction, editable proposals, and scripted approvers used in tests. Hashes the server, tool, and arguments together. | `Proposal::new`, `payload_hash`; `Approver::review`; `ConsoleApprover::decide`; `ScriptedApprover`; `Decision` and `ScriptedStep`. |

### 6.3 Model API and MCP connections

| File | What it is for | Key functions and types |
|---|---|---|
| `agent-client/src/llm/mod.rs` | Groups Claude API types and client implementation. | Declares `anthropic` and `types`. |
| `agent-client/src/llm/types.rs` | Represents model requests, responses, tool definitions, stop reasons, and tool results as Rust/JSON data. | `MessagesRequest::to_body` / `beta_header`; `MessageResponse::stop`, `texts`, `progress_notes`, `tool_uses`; `tool_result_block`. Types include `ToolDefinition`, `Usage`, `MessageResponse`, `StopReason`, and `ToolUse`. |
| `agent-client/src/llm/anthropic.rs` | Calls the Anthropic Messages API over HTTPS and handles timeouts, retries, server errors, and refusals. | `AnthropicClient::new`, `with_retry_policy`, `send`; `send_once`, `backoff`, and `into_error`; `LlmClient` is the shared client interface. |
| `agent-client/src/mcp/mod.rs` | Groups the MCP client and server-launch code. | Declares connection, environment, filesystem, manager, and telemetry modules. |
| `agent-client/src/mcp/child_env.rs` | Builds a safer environment for child MCP processes by excluding sensitive inherited variables. | `filter`, `inherited`. |
| `agent-client/src/mcp/connection.rs` | Starts an MCP child process, completes the protocol handshake, sends requests with timeouts, captures stderr, and shuts it down. | `McpConnection::start`, `list_tools`, `list_resources`, `list_resource_templates`, `call_tool`, `read_resource`, `shutdown`; `ServerLaunch` describes a process to start. |
| `agent-client/src/mcp/manager.rs` | Starts and tracks the set of MCP servers, records which ones are available, and discovers their capabilities. | `ServerSet::connect_all`, `from_connections`, `get`, `discovered`, `unavailable`, `shutdown`; internal `discover`. |
| `agent-client/src/mcp/telemetry.rs` | Creates the launch configuration for the project's Rust telemetry server. | `launch`, `connect`. |
| `agent-client/src/mcp/filesystem.rs` | Launches the external filesystem server and verifies its workspace boundary and read-only tools before use. Refuses unsafe workspace contents or paths. | `launch`, `connect`, `verify_read_only`, `verify_allowed_directories`, `check_workspace`, `canonical`, `path_args`. |

### 6.4 Safety helpers

| File | What it is for | Key functions and types |
|---|---|---|
| `agent-client/src/safety/mod.rs` | Groups safety-specific modules. | Declares `injection` and `redact`. |
| `agent-client/src/safety/injection.rs` | Looks for known instruction-like patterns in returned text and wraps that content with untrusted-data labels. This is a detector, not a general proof that injection is impossible. | `scan`, `wrap_untrusted`. |
| `agent-client/src/safety/redact.rs` | Removes known secrets and local machine paths from trace/display text and nested JSON values. | `Redactor::new`, `with_alias`, `text`, `redact_str`, `json`; internal helpers handle token, key, and private-key patterns. |

## 7. Code reading guide: `mcp-server`

| File | What it is for | Key functions and types |
|---|---|---|
| `mcp-server/Cargo.toml` | Manifest for the telemetry MCP server crate and its dependencies. | Not a Rust source file. |
| `mcp-server/src/main.rs` | Starts the server, loads its settings, builds the data and ticket stores, and connects the server to stdio. | `main`, `run`. |
| `mcp-server/src/lib.rs` | Exposes the server's modules to the binary and tests. | Declares `config`, `data`, `error`, `logging`, `server`, and `tickets`. |
| `mcp-server/src/config.rs` | Parses the fixed or live reference time and resolves data/database paths from environment variables. | `ReferenceTime::parse` / `now`, `ServerConfig::from_env` / `from_lookup`; `ConfigError` describes configuration failures. |
| `mcp-server/src/data.rs` | Defines the mock service data structures; reads JSON/text files; validates search inputs; filters/sorts logs and deployments; loads runbooks and the checklist. | `service_names`, `owner_handles`, `find_service`, `Severity::parse`, `validate_limit`, `validate_window`; `DataStore::search_logs`, `deployment_history`, `runbook`, `release_checklist`, `load_logs`, `load_deployments`; `rfc3339`. |
| `mcp-server/src/error.rs` | Defines structured errors with codes, hints, valid values, and per-field problems. | `ToolError::new`, `with_hint`, `with_valid_values`, `with_field_errors`, `to_json`; types include `ErrorCode` and `FieldError`. |
| `mcp-server/src/logging.rs` | Sets up diagnostic logging without writing ordinary text to protocol stdout. | `init`. |
| `mcp-server/src/server.rs` | Implements MCP discovery, tool calls, resource listing, and resource reads. It connects protocol calls to the data/ticket modules. | `TelemetryServer::new`; tool handlers `search_logs`, `get_deployment_history`, `create_incident_ticket`; resource handlers `list_resources`, `list_resource_templates`, `read_resource`; `all_resource_uris`. Argument types: `SearchLogsArgs`, `DeploymentHistoryArgs`, `CreateTicketArgs`. |
| `mcp-server/src/tickets.rs` | Validates ticket fields and stores/loads tickets in SQLite with generated incident IDs. | `NewTicket::validate`; `TicketStore::new`, `create`, `get`, `count`; ticket ID helpers `format_id`, `parse_id`. Types: `NewTicket`, `Ticket`, `TicketStore`. |

## 8. Test files: what their test functions check

Test functions are named with `#[test]` or `#[tokio::test]`. They are examples of expected behavior, not application functions available to a normal user. Helpers inside test files build fake API responses, temporary databases, or MCP processes.

| File | Test coverage in plain language |
|---|---|
| `agent-client/tests/common/mod.rs` | Shared test utilities: fake local Anthropic HTTP server, scripted LLM, event recorder, and temporary agent/server settings. |
| `agent-client/tests/agent_loop.rs` | MCP discovery, multi-step tool use, error/refusal handling, iteration limits, API failures, and conversation reset. |
| `agent-client/tests/anthropic_http.rs` | HTTP headers/body, parsing, retry rules, rate limits, timeouts, unreachable endpoints, and error messages that avoid exposing the API key. |
| `agent-client/tests/benchmarks_functional.rs` | Seven scripted scenarios: log search, outage evidence, release readiness, unknown service, invalid severity, ambiguous question, and injected log data. |
| `agent-client/tests/cli_e2e.rs` | Runs the real CLI process against a fake model endpoint and real MCP servers; checks interactive commands, approvals, rejection, and saved sample traces. |
| `agent-client/tests/cli_smoke.rs` | Configuration and command-line basics, dotenv/environment precedence, missing-key errors, and `check-mcp` without a key. |
| `agent-client/tests/eval_harness.rs` | Evaluation report and PASS/FAIL/ERROR/NOT_RUN result handling. |
| `agent-client/tests/filesystem_mcp.rs` | Starts the real Node filesystem server; checks allowed files, blocked traversal/writes, read-only mode, startup failures, timeouts, and shutdown. |
| `agent-client/tests/security.rs` | Ticket approval binding, edits and rejection, injected text, filesystem boundaries, tool policy, iteration/tool time limits, server/API failures, redaction, and trace fields. |
| `agent-client/tests/fixtures/misbehaving_mcp_server.js` | Test-only JavaScript child process that behaves incorrectly or hangs so connection failure paths can be exercised. |
| `mcp-server/tests/startup.rs` | Server exits cleanly on disconnect, keeps protocol stdout clean, and reports invalid reference time clearly. |
| `mcp-server/tests/mock_data.rs` | Checks the mock dataset's schemas, service/version/timestamp consistency, scenario facts, workspace allowlist, and absence of real credentials. |
| `mcp-server/tests/mcp_protocol.rs` | Starts the real server and checks tool/resource discovery, validation/errors, log/deployment results, ticket persistence, and recovery from bad data/database conditions. |

Existing `REPORT.md` records a previous run with 197 passed tests, zero failures, and one ignored opt-in test. That result is historical project documentation; tests were not run as part of preparing this guide.

## 9. Mock data and sample runs

| File | What is inside / why it exists |
|---|---|
| `mock_data/logs.json` | 45 simulated log entries for three services, including an outage timeline and one clearly planted instruction-like customer string. |
| `mock_data/deployments.json` | Eight simulated production deployments, versions, commit IDs, and one rollback. |
| `mock_data/release-checklist.md` | Readiness checklist used for release-candidate decisions. |
| `mock_data/runbooks/auth-service.md` | On-call guidance for auth-service issues. |
| `mock_data/runbooks/payments-api.md` | On-call guidance for payment incidents and rollback actions. |
| `mock_data/runbooks/notification-worker.md` | On-call guidance for notification-worker issues. |
| `mock_data/workspace/RELEASE_NOTES.md` | Candidate release notes, served only through the confined filesystem server. |
| `mock_data/workspace/service-config.yaml` | Simulated service configuration, including safe vault-reference placeholders instead of live credentials. |
| `docs/MOCK_DATA_SPEC.md` | Dataset schema and expected scenario findings. It intentionally sits outside the filesystem server's allowed workspace. |
| `evals/benchmarks.json` | Seven benchmark cases with prompts, expected tool patterns, and checks for agent behavior. |
| `evals/results/.gitkeep` | Placeholder that keeps the otherwise empty report-output folder in Git. New evaluation results are generated locally. |
| `traces/samples/README.md` | Explains what each sample trace demonstrates and states that the model responses came from a scripted endpoint. |
| `traces/samples/phase6-scripted-security-demo.jsonl` | Short JSONL example of a scripted security-focused run. |
| `traces/samples/investigation-with-hitl-approval.jsonl` | Example multi-step investigation including a ticket approval and recorded tool results. |
| `traces/samples/investigation-with-hitl-approval.console.txt` | Human-readable console output corresponding to the approval example. |
| `traces/samples/handled-failures-and-edge-cases.jsonl` | Example trace containing rejected or invalid requests and API failure handling. |
| `traces/samples/handled-failures-and-edge-cases.console.txt` | Human-readable console output for those failure cases. |
| `traces/samples/.gitkeep` | Placeholder for the sample trace folder. |

The sample traces use the real agent, real MCP servers, and real safety checks, but their model responses came from a local scripted endpoint. They are not evidence of a live Claude evaluation. The project's evaluation report says no live LLM verdicts were available at the time it was written.

## 10. Documentation files

| File | Beginner description |
|---|---|
| `README.md` | Main run guide: purpose, setup, configuration, CLI examples, tests, troubleshooting, and repository map. Start here if you want to run the project. |
| `REPORT.md` | Detailed design and implementation report, requirement evidence, previous test/evaluation results, known limitations, and development history. |
| `PROJECT_CONTEXT.md` | Project decisions, phase status, assumptions, and development notes for people continuing the work. |
| `docs/PHASE_0_DESIGN.md` | Technical plan: requirements, architecture, components, execution flow, safety, evaluation, and platform choices. |
| `docs/SECURITY.md` | Security boundaries and the protections around tools, ticket approval, untrusted outputs, and trace redaction. |
| `docs/FILESYSTEM_MCP.md` | How the external filesystem server is launched, confined, and checked for read-only access. |
| `docs/EVALUATION.md` | Benchmark definitions and recorded functional/guardrail results, plus the explicit lack of live-model results. |
| `docs/MOCK_DATA_SPEC.md` | Data formats, scenario ground truth, and dataset consistency rules. |
| `docs/learning/phase-0.md` | Beginner introduction to agents, tool calls, MCP, guardrails, traces, and Rust terms. |
| `docs/learning/phase-1.md` | Learning notes for the first implementation phase: Rust workspace, configuration, and server foundation. |
| `docs/learning/phase-2.md` | Learning notes about mock operations data and its validation. |
| `docs/learning/phase-3.md` | Learning notes about the telemetry MCP server and protocol behavior. |
| `docs/learning/phase-4.md` | Learning notes about the read-only filesystem MCP integration. |
| `docs/learning/phase-5.md` | Learning notes about the Claude tool loop and multi-server orchestrator. |
| `docs/learning/phase-6.md` | Learning notes about policy, approvals, injection labeling, redaction, limits, and traces. |
| `docs/learning/phase-7.md` | Learning notes about benchmarks and deterministic testing versus live model quality. |
| `docs/learning/phase-8.md` | Learning notes about documentation, audit evidence, and trace provenance. |

## 11. Scripts and project settings

| File | What it does |
|---|---|
| `scripts/setup.ps1` | Checks Rust/Node prerequisites, installs the pinned Node package, builds Rust crates, prepares `.env`, and runs startup/configuration checks. It can skip or reset the key prompt. |
| `scripts/run.ps1` | Builds the Rust workspace if needed, then starts `agent-client` with the arguments you pass. |
| `scripts/demo-mcp.ps1` | Sends sample raw MCP requests to the Rust telemetry binary, without Claude or an API key. `-Raw` shows JSON protocol messages. |
| `scripts/audit.ps1` | Runs the repository's repeatable build, MCP, test, lint, format, secret-scan, and Git hygiene checks. |
| `Cargo.toml` | Root Cargo workspace and shared Rust dependency/version/lint settings. |
| `Cargo.lock` | Exact resolved Rust dependency versions, making builds more reproducible. Cargo normally updates this when dependencies change. |
| `package.json` | Declares the pinned Node filesystem MCP dependency and the minimum Node version. It contains tooling dependencies, not the Rust application. |
| `package-lock.json` | Exact resolved Node package versions and integrity hashes used by `npm ci`. |
| `.env.example` | Safe configuration template with a placeholder API key and documented default settings. Copy it to `.env` for local use. |
| `.gitignore` | Excludes secrets, generated dependencies/build output, runtime databases, traces, and evaluation output from Git. |
| `.gitattributes` | Defines text line-ending behavior and treats PDFs and databases as binary. |
| `.npmrc` | Requires the Node engine version, blocks install scripts, and keeps package versions exact. |
| `clippy.toml` | Allows `unwrap()` inside Rust tests while production code still gets lint warnings. |
| `.vscode/extensions.json` | Recommends Rust, TOML, and debugger extensions to VS Code users. |
| `.vscode/settings.json` | Sets Rust formatting/lint editor behavior and hides generated folders in VS Code's file view. |
| `data/.gitkeep` | Keeps the runtime data folder in Git; the SQLite ticket database is generated when needed. |

## 12. Configuration a beginner is likely to meet

The safe starting template is `.env.example`. `scripts/setup.ps1` can create a local `.env`; `.gitignore` excludes it. A real API key belongs only in that ignored local file or an environment variable, never in source, a trace, or a report.

| Setting | What it controls |
|---|---|
| `ANTHROPIC_API_KEY` | Authenticates requests to Claude. Needed for normal agent/evaluation calls, but not for the MCP server check. |
| `ANTHROPIC_MODEL` | Model name to request. |
| `ANTHROPIC_EFFORT` | Requested model reasoning effort. |
| `ANTHROPIC_TIMEOUT_SECS` | Maximum wait for one model response. |
| `MAX_ITERATIONS` | Maximum Claude calls in a single user request. |
| `TOOL_TIMEOUT_SECS` | Default maximum wait for one tool call. |
| `TOOL_TIMEOUT_OVERRIDES` | Optional per-tool timeout values. |
| `OPS_REFERENCE_TIME` | Fixed “now” used to interpret time windows over the simulated data. Set to `now` for the real clock. |
| `FILESYSTEM_ROOT` | Filesystem server's permitted folder; default is the mock workspace. |
| `TICKETS_DB` | SQLite file path for persisted tickets. |
| `TRACE_FILE` | JSONL path used for agent traces. |
| `LOG_LEVEL` | Diagnostic detail level; logs are sent to stderr. |

The actual `.env` file is machine-local and was not read while preparing this guide.

## 13. How to build, run, and explore

Commands below match the Windows PowerShell instructions in `README.md` and are run from the project folder.

```powershell
# First-time setup (checks Rust and Node, builds, configures)
powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1

# Ask one question and exit
.\scripts\run.ps1 ask "Show me the last 20 ERROR logs from payments-api."

# Start the interactive CLI
.\scripts\run.ps1

# Check both MCP servers; this does not need an Anthropic key
.\scripts\run.ps1 check-mcp

# Try the telemetry MCP protocol directly, without Claude
powershell -ExecutionPolicy Bypass -File .\scripts\demo-mcp.ps1

# Run a benchmark evaluation (does require a valid API key)
.\scripts\run.ps1 eval
```

In interactive mode, `/tools` lists discovered tools, `/servers` lists MCP servers, `/reset` clears the conversation, `/help` shows commands, and `/exit` ends the session. Ticket approval accepts `a` (approve), `r` (reject), or `e` (edit, then review the new values).

## 14. A practical reading order

If this is your first visit, read these in order:

1. `README.md` for what the project does and how to run it.
2. `docs/learning/phase-0.md` for beginner terms and architecture.
3. `agent-client/src/main.rs` and `agent-client/src/cli.rs` to see how a command becomes a session.
4. `agent-client/src/agent/runner.rs` to follow the model/tool loop.
5. `agent-client/src/agent/registry.rs` and `agent-client/src/agent/validate.rs` to see tool discovery, permissions, and argument validation.
6. `agent-client/src/agent/approval.rs` and `agent-client/src/safety/` to understand the most important safety boundaries.
7. `mcp-server/src/server.rs`, followed by `mcp-server/src/data.rs` and `mcp-server/src/tickets.rs`, to see how tools produce results and persist tickets.
8. The matching integration tests to see examples of expected behavior.
9. `REPORT.md` and `docs/EVALUATION.md` to distinguish verified functional behavior from live model evaluation.

## 15. Scope and limitations of this file map

This guide catalogs the repository's tracked project files and explains their role. Generated folders such as `target/` and `node_modules/` contain build output and installed third-party code, so their many generated files are not listed one by one. `.env` is a local secret-bearing configuration file and was deliberately not opened. The root `Intern Assignment 2.pdf` is ignored by Git and is an assignment reference rather than an implementation file; it is not included in the code map. Runtime databases, generated traces, and evaluation reports are created when the relevant commands run.

The project is a teaching/demo system backed by mock data. The phrase-based injection scan can flag known suspicious text, but should not be understood as a universal prompt-injection solution. The strongest barriers are the restricted tool policy, the read-only filesystem boundary, argument validation, and human approval before writing a ticket. The existing project report also distinguishes functional tests from live Claude reasoning results; consult it for the status recorded by the authors.
