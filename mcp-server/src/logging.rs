//! Diagnostic logging for the MCP server.
//!
//! CRITICAL: with the stdio transport, **stdout carries the MCP protocol**.
//! Anything else printed there corrupts the connection, so every log line goes
//! to **stderr**. `main.rs` also denies `println!` via a clippy lint.
//!
//! Same conventions as the agent's logger: `LOG_LEVEL` (default `info`),
//! overridden by `RUST_LOG`; no ANSI colours.

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

pub fn init(level: &str) -> Result<()> {
    let directives = match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => level.to_string(),
    };
    let filter = EnvFilter::try_new(&directives)
        .with_context(|| format!("invalid log level/filter `{directives}`"))?;

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .compact()
        .try_init()
        .map_err(|e| anyhow::anyhow!("could not initialise logging: {e}"))
}
