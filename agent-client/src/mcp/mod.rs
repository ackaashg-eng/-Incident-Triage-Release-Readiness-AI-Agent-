//! MCP clients: one connection per server, each server its own child process.
//!
//! * `connection`: generic launch / handshake / timeouts / errors / shutdown
//! * `filesystem`: the external `@modelcontextprotocol/server-filesystem` (read-only, confined)
//! * `telemetry`: our `ops-telemetry-mcp`
//! * `child_env`: the scrubbed environment every server starts with

pub mod child_env;
pub mod connection;
pub mod filesystem;
pub mod telemetry;
