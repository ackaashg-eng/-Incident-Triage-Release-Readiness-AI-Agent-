# Learning Notes — Phase 4: Integrating an External MCP Server

## 1. Reuse, don't rebuild
The assignment wants file access through the **official** filesystem server. Our job is not to write file-reading code; it is to **configure, launch, verify and contain** someone else's component. That is most real integration work: you treat the component as a black box with a documented contract, and you check the contract instead of trusting it.

## 2. Reading the source of truth
Before writing code we read the installed package's own README and source (`node_modules\@modelcontextprotocol\server-filesystem`). Things we only learned that way:
* `--read-only` makes the server **not register** write tools at all (safer than refusing calls).
* MCP **Roots** let a client *replace* the allowed folders. `--confine-roots` prevents widening.
* Files are also *resources* (`file://{+path}`), but `resources/list` is deliberately empty.
* Relative paths resolve against the allowed directory, not the process's working directory.
* Windows-specific defences exist (alternate data streams, `NUL`/`CON`, trailing dots) plus one documented gap (a junction race).

## 3. Launching a process safely on Windows
```rust
let mut cmd = tokio::process::Command::new("node");
cmd.args([script, "--read-only", "--confine-roots", "--", workspace])
   .current_dir(workspace)
   .env_clear()                      // start from NOTHING
   .envs(child_env::inherited());    // add back only PATH, SystemRoot, TEMP, ...
```
* **Argument list, no shell.** Each argument goes to the process as-is; a folder called `Agent MCP Ochestra` needs no quoting, and nothing can be "injected" into a command line.
* **`node`, not `npx`.** In PowerShell `npx` is `npx.ps1` (a script), and npx may download code. We run the locked local copy.
* **`env_clear()`.** A child process inherits all your environment variables by default. Our agent loads `ANTHROPIC_API_KEY` from `.env` into its environment, so without this the Node server (and every npm dependency it loads) could read the key. A test proves the child sees only the allow-listed variables.

## 4. Verify after connecting: "trust, but verify"
Flags can be mistyped, a package update can change behaviour. So after the handshake we **check**:
1. every tool is annotated `readOnlyHint: true`, otherwise refuse;
2. `list_allowed_directories` returns exactly our workspace, otherwise refuse.

This is **fail closed**: when in doubt, don't run. The opposite (fail open: "probably fine, continue") is how security incidents start.

## 5. Async Rust you met in this phase
* **`tokio::time::timeout(d, future)`** wraps any future: either it finishes in time, or you get `Err(Elapsed)`. Every MCP request and the startup handshake use it, so a hung server can never hang the agent.
* **`tokio::spawn`** runs a background task: here, reading the server's stderr line by line into a small ring buffer (`VecDeque`, last 20 lines) so error messages can show what the server said before it died.
* **`Arc<Mutex<…>>`** shares that buffer between the background task and the connection.
* **Drop = cleanup.** If a connection value is dropped (an error, a panic), rmcp's child-process guard kills the process. No orphaned `node.exe`. A test checks with `tasklist`.

## 6. Classifying errors so the agent can react
`McpError` separates *what kind* of failure happened:

| Variant | Meaning | What the agent should do (Phase 5) |
|---|---|---|
| `Spawn` | program/package missing | tell the user how to install |
| `StartupTimeout` / `Handshake` | the server didn't start | report, include its stderr |
| `Timeout` | one request too slow | tell Claude the tool timed out |
| `Disconnected` | process died | stop using that server, report |
| `Rejected` | JSON-RPC error (e.g. unknown tool, access denied on a resource) | give Claude the message |
| `Unsafe` | our own safety check failed | refuse to run |

Separately, *tool-level* errors (e.g. "Access denied", "ENOENT") arrive as normal results with `isError: true`. These are expected, safe outcomes that Claude should see.

## 7. Two surprises, caught by tests
* The Node server **omits** `isError` on success (absent means success in MCP). Our first test asserted `Some(false)` and failed. Rule: test `is_error == Some(true)` for failure, never `== Some(false)` for success.
* rmcp negotiated protocol `2025-11-25` with this server, a reminder that client and server agree on a version rather than assuming the newest.

## 8. Library + binary again
`agent-client` is now a library (`lib.rs`) plus `main.rs`, like the server. Integration tests in `tests/` can then call `filesystem::connect(...)` directly, while `agent-client check-mcp` uses the same code for the human-readable report.
