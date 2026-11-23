//! agent-client: command-line entry point.
//!
//! ```text
//! agent-client                  interactive investigation session (needs the API key)
//! agent-client ask "<request>"  one request, then exit (needs the API key)
//! agent-client check-mcp        start, verify and stop both MCP servers (no key needed)
//! agent-client config           show the loaded configuration (key shown as ***)
//! ```

use std::path::Path;
use std::process::ExitCode;

use agent_client::config::{self, Config, McpSettings};
use agent_client::{check, cli, logging};
use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: agent-client [ask \"<request>\" | check-mcp | config]
  (no argument)      interactive investigation session
  ask \"<request>\"    run one request and exit
  check-mcp          start, verify and stop both MCP servers (no API key needed)
  config             show the loaded configuration";

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
    let words: Vec<&str> = args.iter().map(String::as_str).collect();

    match words.as_slice() {
        [] => {
            let config = load(&base_dir)?;
            runtime()?
                .block_on(cli::interactive(&config))
                .map(|()| true)
        }
        ["ask", rest @ ..] if !rest.is_empty() => {
            let question = rest.join(" ");
            let config = load(&base_dir)?;
            runtime()?.block_on(cli::ask(&config, &question))
        }
        ["check-mcp"] => {
            let settings = McpSettings::load(&base_dir)?;
            logging::init(&config::log_level(&|k: &str| std::env::var(k).ok()))?;
            runtime()?.block_on(check::run(&settings))
        }
        ["config"] => {
            let config = load(&base_dir)?;
            print_config(&config);
            Ok(true)
        }
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            Ok(true)
        }
        _ => bail!("unknown arguments {args:?}\n{USAGE}"),
    }
}

fn load(base_dir: &Path) -> Result<Config> {
    let config = Config::load(base_dir)?;
    logging::init(&config.log_level)?;
    tracing::info!(model = %config.model, effort = %config.effort, max_iterations = config.max_iterations, "configuration loaded");
    Ok(config)
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")
}

fn print_config(cfg: &Config) {
    println!("Incident Triage Agent - configuration");
    println!("  model              : {}", cfg.model);
    println!("  effort             : {}", cfg.effort);
    println!("  refusal fallbacks  : {}", on_off(cfg.fallbacks));
    println!("  progress updates   : {}", on_off(cfg.progress_updates));
    println!("  API endpoint       : {}", cfg.api_base_url);
    println!("  API timeout        : {}s", cfg.request_timeout.as_secs());
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
    println!("  tickets database   : {}", cfg.mcp.tickets_db.display());
    println!("  trace file         : {}", cfg.trace_file.display());
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn status(path: &Path) -> &'static str {
    if path.exists() { "found" } else { "missing" }
}
