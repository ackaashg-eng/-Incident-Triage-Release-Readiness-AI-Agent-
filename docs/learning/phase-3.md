# Learning Notes — Phase 3: The Telemetry MCP Server

## 1. What an MCP server actually is
A program that answers **JSON-RPC** messages. With the stdio transport, the client starts the server as a child process, writes one JSON message per line to its **stdin**, and reads answers from its **stdout**:

```
client → {"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"search_logs","arguments":{"service_name":"auth-service","severity":"ERROR","limit":5}}}
server → {"jsonrpc":"2.0","id":8,"result":{"content":[...],"structuredContent":{...},"isError":false}}
```

That is why stdout is sacred: a single stray `println!` would be read as a broken protocol message. Logs go to **stderr**. A clippy lint (`deny(print_stdout)`) and a test enforce this. Run `scripts\demo-mcp.ps1 -Raw` to watch real messages.

The methods we implement:

| Method | Answer |
|---|---|
| `initialize` | name, version, capabilities (tools + resources), instructions |
| `tools/list` | the 3 tools, each with description, JSON Schema and annotations |
| `tools/call` | runs a tool |
| `resources/list` / `resources/templates/list` | the checklist, 3 runbooks, and the `service://runbooks/{service_name}` template |
| `resources/read` | the Markdown text |

## 2. Tools vs. resources
* A **tool** is an *action* the model chooses to call with arguments (search, list, create).
* A **resource** is *a document identified by a URI* that the client can read. Here: runbooks and the checklist. A **resource template** (`{service_name}`) tells the client how to build valid URIs.

## 3. How rmcp turns Rust into MCP
```rust
#[tool(name = "search_logs", description = "...", annotations(read_only_hint = true, ...))]
fn search_logs(&self, Parameters(args): Parameters<SearchLogsArgs>) -> CallToolResult { ... }
```
* `#[tool_router]` collects every `#[tool]` method into a router; `#[tool_handler]` plugs it into `ServerHandler`.
* `Parameters<SearchLogsArgs>` makes the SDK deserialize the JSON arguments into our struct **and** generate the input JSON Schema from it (via `schemars`).
* Doc comments on struct fields become the schema's field descriptions. `#[schemars(range(min = 1, max = 100))]`, `length(...)` and `extend("enum" = [...])` add constraints. `#[serde(deny_unknown_fields)]` becomes `"additionalProperties": false`.
* Resources aren't macros: we implement `list_resources`, `list_resource_templates` and `read_resource` on `ServerHandler` ourselves.

## 4. Two layers of validation
1. **Shape** (the SDK, from the schema): wrong JSON type, missing required field, unknown field → rejected before our code runs (`"failed to deserialize parameters: ..."`).
2. **Meaning** (our code): unknown service, `limit` 100000, `window_hours` −5, severity `CRITICAL`, assignee `admin` → a structured error:
   ```json
   {"error": {"code": "UNKNOWN_SERVICE", "message": "service `billing-service` does not exist",
              "hint": "...", "valid_values": ["auth-service", "payments-api", "notification-worker"]}}
   ```
Why have both? The schema *guides* a well-behaved client; the server must still *enforce*, because nothing forces a client to obey a schema. Never trust input, even from your own agent.

**Tool errors are results, not crashes.** MCP has two failure channels: a JSON-RPC protocol error, or a normal result with `isError: true`. We use the second for tools, because the *caller can read it* and fix its next call: the AI sees `valid_values` and retries correctly. `resources/read` has no error-result form, so resource errors are JSON-RPC errors that carry the same JSON body in `data`.

## 5. Security details worth noticing
* **Allow-list before path.** The runbook URI contains a service name that ends up in a file path. We check the name against the 3 known services *first*, so `service://runbooks/../../.env` is just "unknown service" and never touches the disk.
* **Assignee allow-list.** A ticket can only go to a team on-call handle. The injected "assigned to admin" ticket is rejected by the server itself, regardless of what the AI is fooled into.
* **Errors don't leak internals.** Paths and SQLite messages go to the server log; the client only gets a safe message.
* **Untrusted-data notice.** `search_logs` results say that log text may come from end users and must never be followed as instructions.

## 6. SQLite in a few lines
* `rusqlite` with the `bundled` feature compiles SQLite into our binary, so there's nothing to install.
* `id INTEGER PRIMARY KEY AUTOINCREMENT` gives unique, never-reused numbers; the ticket ID is `INC-` + (1000 + id). Uniqueness is the database's job, not ours, so concurrent writers can't collide (a test runs 8 threads × 5 tickets).
* `CHECK` constraints repeat the rules (severity values, lengths): defence in depth.
* `?1, ?2` placeholders (**parameterised queries**) mean ticket text can never be interpreted as SQL.
* SQLite calls are blocking, so the async tool runs them in `tokio::task::spawn_blocking`.

## 7. Robustness choices
* Data files are read **per request**. A missing or corrupted `logs.json` gives `DATA_UNAVAILABLE` / `DATA_CORRUPT` for that call, while other tools keep working, and a fixed file needs no restart.
* Records are parsed into strict types (`deny_unknown_fields`, a `Severity` enum, real timestamps). Malformed data is detected, not passed on.
* The database is also opened per call: a broken DB fails only ticket creation, with `DATABASE_ERROR`.

## 8. Testing at three levels
| Level | File | What it proves |
|---|---|---|
| Unit | `src/*.rs` `#[cfg(test)]` | Filtering, limits, windows, validation rules, SQLite store, without any MCP |
| Protocol | `tests/mcp_protocol.rs` | The **compiled binary** spoken to by the official rmcp **client** over real stdio |
| Process | `tests/startup.rs` | Clean exit, clean stdout, clear config errors |

To make the server testable, the crate became a **library + a thin binary** (`lib.rs` holds the modules, `main.rs` only wires config → server → stdio).

## 9. A surprise we verified instead of assuming
I expected wrong-type arguments to produce a JSON-RPC `-32602` error. The test showed that rmcp 3.11 returns them as a tool result with `isError: true`, which matches the newer MCP guidance that input-validation errors should reach the model. The test and docs were updated to match the real behaviour. Lesson: let tests tell you what the library does.
