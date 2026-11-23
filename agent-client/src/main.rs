//! agent-client — the Incident Triage & Release Readiness agent (orchestrator).
//!
//! Phase 1: loads and validates the configuration, starts logging and prints
//! a foundation check. The MCP connections and the agent loop come later.
//!
//! Error-handling convention:
//! * each module defines its own typed error enum with `thiserror` (e.g. `ConfigError`);
//! * `main`/`run` use `anyhow` to add context and print one friendly line;
//! * `unwrap()`/`expect()` are not used in application code (clippy warns).

mod config;
mod logging;

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};

use crate::config::Config;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // `{:#}` prints the whole context chain on one line.
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    // The project root is the working directory (scripts\run.ps1 sets it).
    let base_dir = std::env::current_dir().context("cannot determine the current directory")?;

    let config = Config::load(&base_dir)?;
    logging::init(&config.log_level)?;

    tracing::info!(
        model = %config.model,
        effort = %config.effort,
        max_iterations = config.max_iterations,
        "configuration loaded"
    );

    print_foundation_check(&config);
    Ok(())
}

fn print_foundation_check(cfg: &Config) {
    println!("Incident Triage Agent - foundation check (Phase 1)");
    println!("  model              : {}", cfg.model);
    println!("  effort             : {}", cfg.effort);
    println!(
        "  refusal fallbacks  : {}",
        if cfg.fallbacks { "on" } else { "off" }
    );
    println!("  max iterations     : {}", cfg.max_iterations);
    println!("  tool timeout       : {}s", cfg.tool_timeout.as_secs());
    println!("  reference time     : {}", cfg.ops_reference_time);
    println!("  API key            : loaded ({})", cfg.api_key);
    println!(
        "  telemetry server   : {} [{}]",
        cfg.telemetry_server_path.display(),
        status(&cfg.telemetry_server_path)
    );
    println!(
        "  filesystem root    : {} [{}]",
        cfg.filesystem_root.display(),
        status(&cfg.filesystem_root)
    );
    println!("  trace file         : {}", cfg.trace_file.display());
    println!("The agent loop arrives in Phase 5.");
}

fn status(path: &Path) -> &'static str {
    if path.exists() { "found" } else { "missing" }
}
