//! ops-telemetry-mcp: a custom MCP server for simulated operational telemetry.
//!
//! The crate is a library plus a thin binary (`main.rs`), so tests can run the
//! real MCP server in-process as well as the compiled executable.
//!
//! | Module | Role |
//! |---|---|
//! | `server` | MCP layer: tools, input schemas, resources (thin adapter) |
//! | `data` | logs, deployments, runbooks, checklist: loading, validation, filtering |
//! | `tickets` | incident-ticket validation and SQLite storage |
//! | `error` | structured error body returned to clients |
//! | `config`, `logging` | environment settings and stderr logging |
//!
//! Error-handling convention: typed errors inside modules, `anyhow` only in
//! `main`, no `unwrap()` in application code.

// stdout is reserved for MCP protocol messages; `println!` would break clients.
#![deny(clippy::print_stdout)]

pub mod config;
pub mod data;
pub mod error;
pub mod logging;
pub mod server;
pub mod tickets;
