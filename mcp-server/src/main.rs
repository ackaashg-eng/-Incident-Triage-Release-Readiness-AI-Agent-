//! ops-telemetry-mcp — custom MCP server for simulated operational telemetry.
//!
//! Phase 1: reads its configuration, starts stderr logging and exits.
//! The MCP tools and resources are added in Phase 3.
//!
//! Error-handling convention (same as agent-client):
//! typed `thiserror` enums inside modules, `anyhow` with context at the top,
//! no `unwrap()` in application code.

// stdout is reserved for MCP protocol messages; `println!` here would break clients.
#![deny(clippy::print_stdout)]

mod config;
mod logging;

use std::process::ExitCode;

use anyhow::{Context, Result};

use crate::config::ServerConfig;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ops-telemetry-mcp error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let base_dir = std::env::current_dir().context("cannot determine the current directory")?;
    let config = ServerConfig::from_env(&base_dir);
    logging::init(&config.log_level)?;

    tracing::info!(
        data_dir = %config.data_dir.display(),
        tickets_db = %config.tickets_db.display(),
        reference_time = %config.reference_time,
        "ops-telemetry-mcp foundation ready (MCP tools and resources arrive in Phase 3)"
    );
    Ok(())
}
