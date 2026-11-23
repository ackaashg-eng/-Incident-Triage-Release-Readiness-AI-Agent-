//! ops-telemetry-mcp: starts the MCP server on stdio.
//!
//! stdin/stdout carry the MCP protocol (JSON-RPC, one message per line);
//! all diagnostics go to stderr. The server stops when the client closes stdin.

#![deny(clippy::print_stdout)]

use std::process::ExitCode;

use anyhow::{Context, Result};
use ops_telemetry_mcp::config::ServerConfig;
use ops_telemetry_mcp::data::DataStore;
use ops_telemetry_mcp::logging;
use ops_telemetry_mcp::server::TelemetryServer;
use ops_telemetry_mcp::tickets::TicketStore;
use rmcp::ServiceExt;
use rmcp::service::ServerInitializeError;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ops-telemetry-mcp error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let base_dir = std::env::current_dir().context("cannot determine the current directory")?;
    let config = ServerConfig::from_env(&base_dir)?;
    logging::init(&config.log_level)?;

    tracing::info!(
        data_dir = %config.data_dir.display(),
        tickets_db = %config.tickets_db.display(),
        reference_time = %config.reference_time,
        "ops-telemetry-mcp starting on stdio"
    );
    if !config.data_dir.is_dir() {
        // Not fatal: tools report DATA_UNAVAILABLE until the folder exists.
        tracing::warn!(data_dir = %config.data_dir.display(), "data folder not found");
    }

    let server = TelemetryServer::new(
        DataStore::new(config.data_dir, config.reference_time),
        TicketStore::new(config.tickets_db),
    );
    let running = match server.serve(rmcp::transport::stdio()).await {
        Ok(running) => running,
        // The client closed stdin before sending anything: a normal shutdown.
        Err(ServerInitializeError::ConnectionClosed(_)) => {
            tracing::info!(
                "client closed the connection before the session started; shutting down"
            );
            return Ok(());
        }
        Err(err) => return Err(err).context("the MCP session could not start"),
    };
    let reason = running
        .waiting()
        .await
        .context("the MCP session ended with an error")?;
    tracing::info!(?reason, "MCP session closed; shutting down");
    Ok(())
}
