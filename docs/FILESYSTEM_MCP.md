# Filesystem MCP Integration

**Status:** Phase 4, ready for review (2026-11-23)
**Component:** `@modelcontextprotocol/server-filesystem` **2.4.0**, the official reference server, used unchanged as an independent external process.
**Client side:** `agent-client/src/mcp/` (`connection.rs`, `filesystem.rs`, `child_env.rs`)
**Verify it yourself:** `.\scripts\run.ps1 check-mcp` (no API key needed)

---

## 1. Installation (verified)

| Item | Verified value | How |
|---|---|---|
| Node.js | v26.13.1 at `C:\Program Files\nodejs\node.exe` (package requires **>= 24**) | `node --version`, package.json `engines` |
| npm / npx | 11.21.1 | `npm --version`, `npx --version` |
| Package | `@modelcontextprotocol/server-filesystem@2.4.0`, pinned exactly in `package.json`, locked in `package-lock.json` | `npm ci` |
| Entry point | `node_modules\@modelcontextprotocol\server-filesystem\dist\index.js` | package files |

`npm ci` installs exactly the locked versions; `.npmrc` sets `ignore-scripts=true` (no install scripts run) and `engine-strict=true` (refuses an old Node). `scripts\setup.ps1` runs it.

## 2. How it is launched

```
node "<project>\node_modules\@modelcontextprotocol\server-filesystem\dist\index.js" --read-only --confine-roots -- "<project>\mock_data\workspace"
```

| Choice | Why |
|---|---|
| `node <script>` instead of `npx` | In PowerShell, `npx` resolves to `npx.ps1` / `npx.cmd` (a script, not an executable), and `npx` may download packages at run time. Launching `node.exe` with the locked local script avoids both. |
| `--read-only` | The server **does not register** any tool that can write (`write_file`, `edit_file`, `move_file`, `copy_file`, `append_file`, `create_directory`). Calling one gives "tool not found". |
| `--confine-roots` | MCP *Roots* (a deprecated 2025-era feature) would let a client *replace* the allowed directories. With this flag, Roots can only narrow access, never widen it. Our client doesn't send Roots; this is a second safeguard. |
| `--` | Everything after it is a directory, never an option. |
| one directory: `mock_data\workspace` | Canonical absolute path, so `..` tricks and relative paths can't widen it. |
| arguments as a list, no shell | Paths with spaces (`Agent MCP Ochestra`) are passed safely without quoting rules. |
| working directory = workspace | Harmless default. Relative paths in tool calls are resolved by the server against the allowed directory. |
| **scrubbed environment** | The child starts with an empty environment plus 17 allow-listed system variables (PATH, SystemRoot, TEMP, …). `ANTHROPIC_API_KEY`, which the agent loads from `.env`, and `NODE_OPTIONS` never reach Node. |
| stdio transport | No network port; the server lives and dies with the agent. Its stderr is captured into our debug log (last 20 lines kept for error messages). |

## 3. Security controls (defence in depth)

| # | Layer | Control | Fails how |
|---|---|---|---|
| 1 | Before launch | Workspace must exist, be a folder, not be the project root or contain it, not be a drive root | refuse to start |
| 2 | Before launch | Workspace scanned (links not followed): no `.env*`, `*.pem/.key/.pfx/.p12/.db/.sqlite/.kdbx`, no symlinks/junctions | refuse to start |
| 3 | Launch | `--read-only`, `--confine-roots`, single allowed directory, scrubbed environment | — |
| 4 | Server (upstream) | Every path resolved (incl. symlinks) and checked against the allowed directory; Windows: NTFS alternate streams, device names (`NUL`, `CON`…), trailing dots/spaces, UNC paths outside → "Access denied" | error result |
| 5 | After connect | Every offered tool must be annotated `readOnlyHint: true` | disconnect + refuse |
| 6 | After connect | `list_allowed_directories` must report exactly the workspace | disconnect + refuse |
| 7 | Every request | Timeout (default 10 s); a dead server is reported as *disconnected*, never a hang | `McpError` |
| 8 | Phase 5/6 | File contents are **untrusted data**: they will be wrapped and labelled as such for Claude, and scanned for injection patterns | — |

What the agent can see: exactly two files, `RELEASE_NOTES.md` and `service-config.yaml`. A Phase 2 test keeps the workspace at exactly these two files. Secrets in `service-config.yaml` are `${vault:…}` references only. The answer key (`docs/MOCK_DATA_SPEC.md`) lies outside the workspace and was tested as unreadable.

## 4. Discovered capabilities

| Kind | Items |
|---|---|
| Tools (10, all `readOnlyHint: true`) | `read_text_file`, `read_file` (deprecated alias), `read_media_file`, `read_multiple_files`, `list_directory`, `list_directory_with_sizes`, `directory_tree`, `search_files`, `get_file_info`, `list_allowed_directories` |
| Resource template | `file://{+path}` (e.g. `file:///C:/…/Agent%20MCP%20Ochestra/mock_data/workspace/RELEASE_NOTES.md`) |
| Resource list | empty by design: the server never enumerates the disk |
| Protocol | `2025-11-25` negotiated with rmcp 3.11 |

## 5. Test documentation

### 5.1 Automated (`cargo test --workspace`)
`agent-client/tests/filesystem_mcp.rs` runs the real Node server, 15 tests:

| Required test | Test(s) | Result |
|---|---|---|
| Server startup | `starts_connects_and_discovers_only_read_only_tools` | pass |
| MCP connection | same + `the_only_allowed_directory_is_the_workspace` | pass |
| Tool discovery | exactly the 10 read-only tools | pass |
| Resource discovery | `resource_discovery_offers_the_file_template_and_lists_nothing` | pass |
| Read release notes | `reads_release_notes_and_service_config` (relative, absolute with spaces, `file://` resource) | pass |
| Read service config | same | pass |
| Access outside the workspace | `access_outside_the_workspace_is_denied`: 13 paths (`..\..\.env`, `../../.env`, root `.env`, `.env.example`, `Cargo.toml`, `mock_data\logs.json`, `docs\MOCK_DATA_SPEC.md`, `C:\Windows\win.ini`, `~\.ssh\id_rsa`, `\\localhost\C$\…`, `\\?\C:\…`, `RELEASE_NOTES.md::$DATA`, `NUL`) + `list_directory ..`, `directory_tree <root>`, `get_file_info <root>\.env`, `search_files ../**/.env`, `resources/read` outside | pass (all denied) |
| No write capability | `write_tools_do_not_exist` (6 write tools → "not found"; no file created) | pass |
| Missing files | `a_missing_file_is_an_error_result` (ENOENT error result; connection still usable) | pass |
| Server disconnection | `a_killed_server_is_reported_as_disconnected_not_a_hang` (`taskkill /F` → `Disconnected` immediately) | pass |
| Shutdown | `shutdown_stops_the_node_process` (PID gone per `tasklist`) | pass |
| Startup errors | missing `node.exe` → clear spawn error; missing package → "run npm ci"; unsafe workspace refused before launch; non-MCP program → handshake error with its stderr; silent program → startup timeout (2 s) | pass |
| Environment | `servers_start_with_a_scrubbed_environment` (child sees only allow-listed variables) | pass |

Plus `cli_smoke.rs::check_mcp_passes_in_the_project_folder_without_a_key` runs the whole `check-mcp` report.

Unit tests (`src/mcp/*.rs`): workspace checks (clean accepted; root/parent/missing refused; `.env`, `.env.local`, `*.pem`, nested `*.db`, `*.KEY` refused), read-only verification (missing or false annotation fails closed; empty tool list fails), allowed-directory verification, environment filter.

### 5.2 Manual: `.\scripts\run.ps1 check-mcp`
Prints one `[ok]`/`[FAIL]` line per expectation and exits 1 on any failure. Verified 2026-11-23: **all checks passed**, no `node.exe` left running afterwards.

## 6. Observed behaviour worth knowing (for Phase 5)
* **Success results omit `isError`** (absent ⇒ success, per spec). Code must test `is_error == Some(true)`, never `== Some(false)`.
* **Errors are tool results**, e.g. `Access denied - path outside allowed directories: <path> not in <allowed>` or `ENOENT: …`; unknown tools and outside `resources/read` are JSON-RPC errors (-32602).
* Error messages contain **absolute paths** (including the Windows user folder). Harmless locally; Phase 6 may shorten them before they go to Claude.

## 7. Known limitations
* **Windows junction race (upstream, documented by the package):** a *different local process* that can already write inside the workspace could swap a parent folder for a junction between the server's check and its open. Mitigation here: the workspace is a dedicated project folder no untrusted process writes to; the pre-launch scan refuses links; the server is read-only, so it can't create links itself.
* The workspace scan runs at startup; files added while the agent runs are not re-scanned (the server still confines every access).
* Node.js and the package are trusted third-party code; they are pinned (`package-lock.json`) and run without our secrets.
