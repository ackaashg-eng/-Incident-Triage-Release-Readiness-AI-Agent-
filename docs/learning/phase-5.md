# Learning Notes — Phase 5: The Multi-Server Agent Orchestrator

## 1. What "agentic loop" really means
The model never runs anything. It only *asks*. The loop is ordinary code:

```
send(system, tools, history) → response
if response has tool_use blocks:
    run each one (or refuse it) → tool_result blocks
    append [assistant response, user(tool_results)] to history
    repeat
else:
    the text is the answer
```
Claude decides **what** to call; our code decides **whether** it may run, **runs** it, and **hands back** the result. That split is the whole security model: the model has no direct path to any server.

## 2. Four rules of the Messages API tool loop
1. **Every `tool_use` gets exactly one `tool_result`**, matched by `tool_use_id`, all in **one** user message, in order. Claude may ask for several tools in one response (we test that).
2. **Errors are results too.** Unknown tool, bad arguments, access denied: we answer with `is_error: true` and a clear message, so Claude can correct itself. The loop never crashes on a bad request.
3. **Check `stop_reason` before running tools.** `refusal` (a safety classifier declined) and `max_tokens` (cut off) can leave a `tool_use` half-written. We never execute tools from such a reply.
4. **The history is append-only.** Opus 5.5 "preserved thinking" ties its (hidden) reasoning blocks to the exact conversation, so assistant content is stored as raw JSON and sent back byte-for-byte. The system prompt and tool list never change mid-session. When a turn fails, we *cut the history back* to where the turn started; that keeps the earlier prefix identical.

## 3. Dynamic discovery → a registry
At startup both MCP servers are launched **concurrently** (`tokio::join!`) and asked for their tools, resources and templates. Each tool becomes a `RegisteredTool`:

| Field | Example |
|---|---|
| name Claude sees | `telemetry__search_logs` (server prefix = owner) |
| owning server / MCP name | `telemetry` / `search_logs` |
| description | `[telemetry server \| read-only] Search the application logs of ONE service…` |
| original MCP schema | kept for validation |
| adapted schema | what Claude receives |
| permission | read-only / requires approval / blocked |

The system prompt names **no tools**. Add a tool to a server and the agent offers it automatically, if the policy allows it.

**Permission policy (fail closed):** a tool annotated `readOnlyHint: true` → read-only; on the explicit approval list (`create_incident_ticket`) → requires approval; anything else → *blocked*, never shown to Claude.

**Resources become a tool.** Claude can only call tools, so for a server that lists resources we generate `telemetry__read_resource`, whose description lists the discovered URIs and templates. URIs are checked against discovery before the request goes out (`service://runbooks/../../.env` never leaves the agent).

## 4. Why adapt schemas, and why still validate
MCP servers publish full JSON Schema *documents* (`$schema`, sometimes no `type`). Claude needs an *object* schema, so we drop document keywords, ensure `type: "object"` and `properties`, and keep everything else (enums, ranges, descriptions). Claude usually respects the schema, but nothing *forces* it to. So before calling a server we validate the arguments against the **original** schema (`validate.rs`): a wrong call never reaches a server, and Claude gets every problem in one message.

## 5. Talking to Claude without an SDK
There is no official Anthropic SDK for Rust, so `llm/anthropic.rs` speaks HTTPS directly (reqwest + Windows SChannel TLS):
* headers `x-api-key`, `anthropic-version: 2026-10-21`, `anthropic-beta` (fallbacks + progress updates);
* Opus 5.5 specifics: no `temperature`; no `thinking` switch (always on), depth via `output_config.effort`; `tool_choice` only `auto`; the system prompt marked for **prompt caching**;
* **retries** for 429 (rate limit), 5xx, 529 (overloaded), network errors and timeouts, at most 3, honouring `retry-after`, else exponential backoff with jitter; 400/401/403/404 are **not** retried (they won't fix themselves);
* the key goes only into the header; `ApiKey` prints as `***`, and tests check it never appears in output.

## 6. Testing an AI agent without an AI
Three test doubles keep everything verifiable with no key and no network:

| Double | Tests |
|---|---|
| `ScriptedLlm` (implements `LlmClient`) | the loop against the **real** MCP servers: parallel calls, sequential steps, unknown tools, invalid args, refusals, truncation, API errors, iteration limit, history shape |
| `FakeAnthropic` (a tiny local HTTP server) | the real HTTP client: headers, body, retries with `retry-after`, error mapping, timeouts, malformed JSON |
| `FakeAnthropic` + the real binary | the CLI end to end (`ask` and interactive mode) |

The `LlmClient` trait is the seam: production uses `AnthropicClient`, tests use the script. This is **dependency injection** in its simplest form.

## 7. Bugs the tests caught
* The filesystem server sends a file both as text and as `structuredContent: {"content": "..."}`. Preferring structured content turned files into one escaped string. Fix: prefer the text parts.
* On Windows, connecting to a closed localhost port isn't refused straight away; the TCP stack retries for ~2 s. A test assumed an instant error.
* **Ctrl+C on Windows reaches every process on the console**, so cancelling one request would have killed both MCP servers. They now start in their own process group.

## 8. What Phase 6 adds
The `Approver` trait is the hand-off point. Phase 5's `DeferredApprover` shows the proposal and runs nothing. Phase 6 plugs in the interactive Approve / Edit / Reject gate (hash-bound to the exact arguments), injection detection on tool results, and JSONL traces.
