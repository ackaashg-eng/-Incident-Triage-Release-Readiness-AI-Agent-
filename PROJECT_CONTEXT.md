# PROJECT_CONTEXT

> Read this first at the start of every session, then verify the real repository state; this file can be wrong.

## Objective
Incident Triage & Release Readiness AI Agent: a Rust CLI orchestrator that uses Claude (Messages API) and two MCP servers (custom Rust `ops-telemetry-mcp` + official filesystem server) to investigate simulated incidents, check release readiness, and create incident tickets only after human approval. Spec: `Intern Assignment 2.pdf` + master instructions + phase prompts.

## Location & Git
`C:\Users\ASUS\Music\SpilLabs\Agent MCP Ochestra` (name kept as-is by user decision).
Own git repo since Phase 1 (`git init -b main`; `git rev-parse --show-toplevel` = this folder). Phase 1 committed on `main` (5 commits, including `package-lock.json`). The author identity is set for this repo only (`ackaashg`, the user's account email, by the user's choice). Never use the repo at `C:\Users\ASUS`.

## Phase status
| Phase | Name | Status |
|---|---|---|
| 0 | Analysis & architecture planning | COMPLETE (approved 2026-11-23) |
| 1 | Environment & project foundation | READY FOR REVIEW (committed; API key deferred by the user) |
| 2 | Mock data engineering | NOT STARTED |
| 3 | Custom telemetry MCP server | NOT STARTED |
| 4 | Filesystem MCP integration | NOT STARTED |
| 5 | Multi-server agent orchestrator | NOT STARTED |
| 6 | Guardrails & security | NOT STARTED |
| 7 | Evaluation & testing | NOT STARTED |
| 8 | Documentation & submission | NOT STARTED |

**Current phase:** 1, READY FOR REVIEW.
**Next approved action:** none. Phase 2 starts only after the user approves Phase 1.

## Confirmed decisions
| Topic | Decision | Source |
|---|---|---|
| Language | Rust (server + orchestrator) | master |
| LLM | Anthropic Messages API, raw HTTPS via reqwest (no official Rust SDK) | Phase 0 |
| Model | `claude-opus-5-5` default (`ANTHROPIC_MODEL`), effort `medium`, refusal fallbacks on (switchable) | Phase 0 |
| Key | `.env`; `scripts\setup.ps1` asks for it with hidden input and writes UTF-8 without a BOM | user + Phase 1 |
| Repo | own git repo here; the user creates the GitHub repo; we add the remote in Phase 8 | user |
| Layout | root Cargo workspace; tests inside each crate; `data/ evals/ scripts/ docs/ traces/samples/` | Phase 0 |
| Interface | interactive CLI; web UI only after the CLI is approved | master |
| Filesystem | read-only agent: `--read-only`, root = `mock_data/workspace` | user |
| Approval | Approve / Edit / Reject; edit → re-approval; hash-bound | user |
| Memory | session-only; `/reset` clears; nothing saved between runs | user |
| Teaching notes | `docs/learning/phase-N.md` | user |
| MCP SDK | `rmcp =3.11.0` | Phase 0 |
| Transport | stdio for both servers | Phase 0 |
| Tickets | SQLite (`rusqlite`, bundled) at `data/tickets.db` | Phase 0 |
| FS server | `@modelcontextprotocol/server-filesystem@2.4.0`; **requires Node ≥ 24**; the user chose to upgrade Node (not downgrade the server) | Phase 1 decision |
| Time anchor | seed data around 2026-11-20T12:00:00Z; `OPS_REFERENCE_TIME` | Phase 0 |
| Config loading | only `<cwd>\.env` (no parent search); real env vars override `.env`; parsing via lookup fn (testable) | Phase 1 |
| Errors | `thiserror` enums per module; `anyhow` + context in `main`; no `unwrap` in app code (clippy) | Phase 1 |
| Logging | `tracing` → **stderr**, no ANSI; `LOG_LEVEL` (agent default `warn`, server `info`), `RUST_LOG` overrides; server denies `println!` (clippy) | Phase 1 |
| Assignment PDF | git-ignored (company document) | Phase 1 |
| reqwest TLS | decided in Phase 5 (rustls → aws-lc-rs may need CMake/NASM; native-tls/SChannel needs none) | Phase 1 |

Full design: `docs/PHASE_0_DESIGN.md`.

## Directory structure (actual, end of Phase 1)
```
Cargo.toml  Cargo.lock  clippy.toml  package.json  .npmrc
.env.example  .gitignore  .gitattributes  .vscode/{extensions,settings}.json
README.md  PROJECT_CONTEXT.md
agent-client/  Cargo.toml  src/{main,config,logging}.rs  tests/cli_smoke.rs
mcp-server/    Cargo.toml  src/{main,config,logging}.rs  tests/startup.rs
mock_data/{runbooks,workspace}/.gitkeep   data/.gitkeep   evals/results/.gitkeep   traces/samples/.gitkeep
scripts/{setup,run}.ps1
docs/PHASE_0_DESIGN.md  docs/learning/{phase-0,phase-1}.md
.env (local only, placeholder key; git-ignored)
```

## Verified environment (2026-11-23)
- rustc / cargo 1.99.1 (stable-x86_64-pc-windows-msvc), with clippy and rustfmt
- Visual Studio Build Tools 2026 (VC tools x64)
- node **v26.13.1** (the user upgraded from 22.18 during Phase 1), npm/npx 11.x
- git 2.51.0; PowerShell 5.1.26100; execution policy CurrentUser = RemoteSigned
- VS Code 1.147.0; Rust extensions not installed (recommended via `.vscode/extensions.json`)
- gh CLI not installed (not needed)

## Dependency versions
In use (Cargo.lock): anyhow 1.0.105 · thiserror 2.0.21 · dotenvy 0.15.7 · tracing 0.1.44 · tracing-subscriber 0.3.23 · tempfile 3.27.0 (dev).
Declared for later phases (workspace table, not compiled yet): rmcp =3.11.0 · tokio 1.54.1 · serde 1 · serde_json 1.0.154 · schemars 1.2.3 · rusqlite 0.43.0 · chrono 0.4.45 · reqwest 0.13.5 · clap 4.6.9 · sha2 0.11.1.
npm (pinned + `package-lock.json` committed, 12 packages): @modelcontextprotocol/server-filesystem 2.4.0.

## Verified commands (run from the project folder)
| Command | Result (2026-11-23) |
|---|---|
| `cargo build --workspace` | OK |
| `cargo test --workspace` | 16 passed, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | OK, no warnings |
| `cargo fmt --all -- --check` | OK |
| `powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1 -SkipKey` | exit 1 as expected: open item "Node 24+"; the build and `.env` creation worked |
| `.\scripts\run.ps1` (placeholder key) | friendly error, exit 1 |
| `.\scripts\run.ps1` (key from the environment) | foundation check printed, key shown as `***`, exit 0 |
| `git check-ignore -v .env target data/tickets.db …` | all ignored; `.env.example` not ignored |

| `npm ci` (Node 26.13.1) | OK, 12 packages, server-filesystem 2.4.0 |
| `node node_modules\@modelcontextprotocol\server-filesystem\dist\index.js --read-only "<abs>\mock_data\workspace"` | "Read-only mode" printed; the only allowed dir is `mock_data\workspace`; stdout empty |

**Not verified by me:** the interactive key prompt in `setup.ps1` (needs a human at the keyboard and an API key, which the user doesn't have yet).

## Test results (Phase 1)
- agent-client unit (config): 8/8
- agent-client integration (cli_smoke): 5/5. Missing key; key never printed (debug logging on); env overrides `.env`; `.env.example` placeholder-only and parseable; invalid setting named
- mcp-server unit (config): 2/2
- mcp-server integration (startup): 1/1 (stdout stays empty)

## Known issues / open items
- **No API key yet** (the user will get one later). Everything up to Phase 4 works without it (tests need no key). It's needed for the first live agent run in Phase 5; then run `scripts\setup.ps1` (or `-ResetKey`).
- GitHub repo: the user provides the URL in Phase 8.
- rmcp 3.11.0 is new: compile probe at the start of Phase 3.
- The telemetry server's `OPS_REFERENCE_TIME` is not validated until Phase 3 (needs chrono).

## Security considerations
Design §9. Phase 1 controls in place: `.env` ignored; `.env.example` placeholder-only (tested); key wrapped in `ApiKey` with redacting Debug/Display (tested incl. debug logs); hidden key entry in setup; `.npmrc` ignore-scripts + engine-strict; `unsafe_code` forbidden; stdout of the MCP server reserved for the protocol (lint + test).
