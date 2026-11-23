//! agent-client: command-line entry point.
//!
//! ```text
//! agent-client             foundation check (needs the API key)
//! agent-client check-mcp   start both MCP servers, verify them, stop them (no key needed)
//! ```
//! The interactive agent arrives in Phase 5.

use std::path::Path;
use std::process::ExitCode;

use agent_client::config::{self, Config, McpSettings};
use agent_client::{check, logging};
use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: agent-client [check-mcp]\n  (no argument)  foundation check\n  check-mcp       start, verify and stop both MCP servers (no API key needed)";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            // `{:#}` prints the whole context chain on one line.
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    // The project root is the working directory (scripts\run.ps1 sets it).
    let base_dir = std::env::current_dir().context("cannot determine the current directory")?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => foundation_check(&base_dir).map(|()| true),
        ["check-mcp"] => check_mcp(&base_dir),
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            Ok(true)
        }
        _ => bail!("unknown arguments {args:?}\n{USAGE}"),
    }
}

fn check_mcp(base_dir: &Path) -> Result<bool> {
    let settings = McpSettings::load(base_dir)?;
    logging::init(&config::log_level(&|k: &str| std::env::var(k).ok()))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?;
    runtime.block_on(check::run(&settings))
}

fn foundation_check(base_dir: &Path) -> Result<()> {
    let config = Config::load(base_dir)?;
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
    println!("Incident Triage Agent - foundation check");
    println!("  model              : {}", cfg.model);
    println!("  effort             : {}", cfg.effort);
    println!(
        "  refusal fallbacks  : {}",
        if cfg.fallbacks { "on" } else { "off" }
    );
    println!("  max iterations     : {}", cfg.max_iterations);
    println!("  tool timeout       : {}s", cfg.mcp.tool_timeout.as_secs());
    println!("  reference time     : {}", cfg.mcp.ops_reference_time);
    println!("  API key            : loaded ({})", cfg.api_key);
    println!(
        "  telemetry server   : {} [{}]",
        cfg.mcp.telemetry_server_path.display(),
        status(&cfg.mcp.telemetry_server_path)
    );
    println!(
        "  filesystem server  : {} [{}]",
        cfg.mcp.filesystem_server_script.display(),
        status(&cfg.mcp.filesystem_server_script)
    );
    println!(
        "  filesystem root    : {} [{}]",
        cfg.mcp.filesystem_root.display(),
        status(&cfg.mcp.filesystem_root)
    );
    println!("  trace file         : {}", cfg.trace_file.display());
    println!(
        "Run `agent-client check-mcp` to test both MCP servers. The agent loop arrives in Phase 5."
    );
}

fn status(path: &Path) -> &'static str {
    if path.exists() { "found" } else { "missing" }
}
