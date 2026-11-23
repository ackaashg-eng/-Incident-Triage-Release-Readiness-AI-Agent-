//! Diagnostic logging (developer-facing), built on `tracing`.
//!
//! * Logs go to **stderr**, so they never mix with the CLI conversation on stdout.
//! * The level comes from `LOG_LEVEL` (default `warn`, which keeps the CLI quiet);
//!   `RUST_LOG`, if set, overrides it and accepts full filter directives
//!   such as `agent_client=debug`.
//! * No ANSI colour codes: Windows PowerShell 5.1 consoles may show them as garbage.
//!
//! Not to be confused with *traces* (`traces/agent_traces.jsonl`), the audit
//! record of tool calls that arrives in Phase 5.

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
