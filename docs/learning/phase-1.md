# Learning Notes — Phase 1: Project Foundation

This phase built the "skeleton" every later phase hangs code on: a Cargo workspace, configuration, logging, error handling, tests, scripts and Git hygiene.

---

## 1. Cargo workspace

**What:** one root `Cargo.toml` that groups several crates (Rust packages) so they build together.

```toml
[workspace]
resolver = "3"
members = ["mcp-server", "agent-client"]
```

**Why:** one command (`cargo build --workspace`) builds both programs, they share one `Cargo.lock` (identical dependency versions), and one `target/` folder (no duplicate compiling).

**`[workspace.dependencies]`** is a central price list of versions. A crate opts in with:

```toml
tracing.workspace = true
```

A crate only compiles the dependencies it actually lists, so planned entries for later phases (rmcp, tokio, …) cost nothing yet.

**`[workspace.lints]`** sets shared code rules. Ours:
* `unsafe_code = "forbid"`: we never need `unsafe`, so it's banned outright.
* `clippy::unwrap_used = "warn"`: `unwrap()` crashes the program on an error. Application code must handle errors instead (`clippy.toml` still allows it inside tests).

---

## 2. Two binaries, two crates

| Folder | Crate name | Produces |
|---|---|---|
| `mcp-server/` | `ops-telemetry-mcp` | `target\debug\ops-telemetry-mcp.exe` |
| `agent-client/` | `agent-client` | `target\debug\agent-client.exe` |

Each has `src/main.rs` (the entry point) plus modules. `mod config;` in `main.rs` tells Rust "there is a file `config.rs` next to me, include it as the module `config`".

---

## 3. Configuration (`agent-client/src/config.rs`)

### `.env` files
A `.env` file is a plain list of `NAME=value` lines. `dotenvy` reads it into the process's environment variables at startup. Two deliberate choices:

1. We read **only** `.env` in the current folder (no searching parent folders, where a stray `.env` could be picked up).
2. **Real environment variables win.** `dotenvy` never overwrites a variable that is already set, so `$env:ANTHROPIC_MODEL = "x"` in PowerShell beats `.env`. A test proves this.

### Parsing against a "lookup function"
```rust
pub fn from_lookup(base_dir: &Path, lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError>
```
* `impl Fn(&str) -> Option<String>` means "any function that takes a variable name and returns its value, if there is one".
* At runtime we pass `|name| std::env::var(name).ok()`, which reads the real environment.
* In tests we pass a `HashMap` lookup instead.

**Why:** in Rust 2024, changing environment variables (`std::env::set_var`) is `unsafe` because tests run in parallel threads. This design avoids the problem entirely.

### `Result<T, E>` and `?`
Rust has no exceptions. A function that can fail returns `Result`: either `Ok(value)` or `Err(error)`.
`?` means: "if this is an `Err`, return it from my function right now; otherwise give me the value."

```rust
let effort = match get("ANTHROPIC_EFFORT") {
    None => Effort::Medium,                        // not set -> default
    Some(v) => v.parse().map_err(|reason| ConfigError::InvalidValue { ... })?,
};
```

### Hiding the API key: the newtype pattern
```rust
pub struct ApiKey(String);          // a "wrapper" type around String
impl fmt::Debug for ApiKey { ... f.write_str("ApiKey(***)") }
```
The inner `String` is private, and printing the key in any form shows `***`. The only way to get the real value is `api_key.expose()`, which is easy to search for in a code review. Tests check that the key never appears in `{:?}` output, in stdout, or in stderr even with debug logging on.

### Enums for fixed choices
```rust
pub enum Effort { Low, Medium, High, XHigh, Max }
impl FromStr for Effort { ... }   // lets us write "high".parse::<Effort>()
```
An invalid string can't become an `Effort`; the compiler guarantees later code only ever sees valid values.

---

## 4. Error-handling convention

| Where | Tool | Why |
|---|---|---|
| Inside modules | `thiserror` enums, e.g. `ConfigError` | Each failure is a named case the code (and tests) can match on |
| At the top (`main` → `run`) | `anyhow::Result` + `.context("...")` | Adds "what we were doing" and prints one readable line |

```rust
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("ANTHROPIC_API_KEY is not set. Run `scripts\\setup.ps1` ...")]
    MissingApiKey,
    ...
}
```
`#[error("...")]` generates the message. Every message tells the user **how to fix** the problem.

`main` returns an `ExitCode`: 0 = success, 1 = failure. Scripts and tests check it.

---

## 5. Logging vs. printing vs. tracing

| Channel | Used for | Destination |
|---|---|---|
| `println!` | The conversation with the user (agent only) | stdout |
| `tracing::info!` and similar | Developer diagnostics | **stderr** |
| Trace file (Phase 5) | Audit record of every tool call | `traces/agent_traces.jsonl` |

`tracing` logs are **structured**: values are attached as named fields, not glued into text.
```rust
tracing::info!(model = %config.model, max_iterations = config.max_iterations, "configuration loaded");
```
`%` means "format this with `Display`".

**Why stderr matters for the MCP server:** with the stdio transport, the server's stdout *is* the protocol channel. One stray `println!` corrupts it. So:
* the server logs only to stderr;
* `#![deny(clippy::print_stdout)]` in `mcp-server/src/main.rs` makes clippy refuse any `println!`;
* the test `starts_and_keeps_stdout_clean` checks that stdout stays empty.

---

## 6. Tests

| Kind | Where | Example |
|---|---|---|
| Unit tests | `#[cfg(test)] mod tests` at the bottom of a source file | `config.rs`: defaults, invalid values, key hiding |
| Integration tests | `<crate>/tests/*.rs`, which run the **compiled program** | `cli_smoke.rs`: missing key → friendly error; `.env.example` parses |

`env!("CARGO_BIN_EXE_agent-client")` gives a test the path of the freshly built binary. Each test runs it inside a `tempfile::tempdir()`, a throwaway folder, so your real `.env` is never touched.

---

## 7. Windows and PowerShell details

* **Execution policy:** PowerShell may refuse to run `.ps1` files. `powershell -ExecutionPolicy Bypass -File script.ps1` allows it for that one run.
* **UTF-8 without BOM:** PowerShell 5.1's `Set-Content -Encoding utf8` adds an invisible byte-order mark (BOM) at the start of the file. In `.env` that would turn the first name into `\uFEFFANTHROPIC_API_KEY`. `setup.ps1` writes with `New-Object UTF8Encoding($false)` (no BOM).
* **Hidden input:** `Read-Host -AsSecureString` doesn't echo what you type. The key is converted to text only to write `.env`, then the variable is removed.
* **`Push-Location` / `Pop-Location`:** the scripts switch to the project folder and always switch back (`finally`), so your shell isn't left somewhere else.
* **Paths with spaces** (`Agent MCP Ochestra`): commands receive paths as separate arguments, never glued into a shell string, so spaces are safe.
* **`$LASTEXITCODE`:** after running a program (cargo, npm), PowerShell stores its exit code here. The scripts check it after every step.

---

## 8. Git hygiene

* `git init` created a repository **in this folder**, separate from the one at `C:\Users\ASUS`. `git rev-parse --show-toplevel` confirms the root.
* `.gitignore`: `.env`, `target/`, `node_modules/`, `data/*.db`, the live trace file, the eval results and the assignment PDF are never committed.
* `.env.example` *is* committed, with placeholders only. A test fails if it ever contains `sk-ant-`.
* `.gitattributes` keeps line endings consistent (LF in the repo, CRLF for `.ps1`).
* `.npmrc`: `engine-strict` (refuse a wrong Node version), `ignore-scripts` (dependencies can't run code at install time), `save-exact` (exact versions).
