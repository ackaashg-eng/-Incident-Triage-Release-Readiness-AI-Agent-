//! agent-client: the Incident Triage & Release Readiness agent (orchestrator).
//!
//! A library plus a thin binary (`main.rs`), so integration tests can drive
//! the MCP connections directly.
//!
//! | Module | Role |
//! |---|---|
//! | `config` | `.env` + environment → validated settings (`Config`, `McpSettings`) |
//! | `logging` | diagnostics on stderr |
//! | `mcp` | MCP client connections (filesystem + telemetry servers) |
//! | `llm` | Claude Messages API client |
//! | `agent` | tool registry, validation, prompt, approval hand-off, agentic loop |
//! | `cli` | interactive session and `ask` mode |
//! | `check` | the `check-mcp` command |
//!
//! Error-handling convention: typed `thiserror` enums inside modules, `anyhow`
//! with context at the top, no `unwrap()`/`expect()` in application code.

pub mod agent;
pub mod check;
pub mod cli;
pub mod config;
pub mod eval;
pub mod llm;
pub mod logging;
pub mod mcp;
pub mod safety;
pub mod trace;
