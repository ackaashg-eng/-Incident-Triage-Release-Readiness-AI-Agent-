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
//! | `check` | the `check-mcp` command |
//!
//! Error-handling convention: typed `thiserror` enums inside modules, `anyhow`
//! with context at the top, no `unwrap()`/`expect()` in application code.

pub mod check;
pub mod config;
pub mod logging;
pub mod mcp;
