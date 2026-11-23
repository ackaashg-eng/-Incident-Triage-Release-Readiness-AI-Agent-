//! Launch settings for our own `ops-telemetry-mcp` server (built by cargo).

use super::connection::{McpConnection, McpError, ServerLaunch};
use crate::config::McpSettings;

pub const SERVER_NAME: &str = "telemetry";

pub fn launch(settings: &McpSettings) -> Result<ServerLaunch, McpError> {
    let exe = &settings.telemetry_server_path;
    if !exe.is_file() {
        return Err(McpError::Spawn {
            server: SERVER_NAME,
            program: exe.display().to_string(),
            reason: "the telemetry server is not built; run `cargo build --workspace`".to_string(),
        });
    }
    let root = &settings.project_root;
    Ok(ServerLaunch {
        name: SERVER_NAME,
        program: exe.clone(),
        args: Vec::new(),
        cwd: root.clone(),
        env: vec![
            (
                "DATA_DIR".to_string(),
                root.join("mock_data").display().to_string(),
            ),
            (
                "TICKETS_DB".to_string(),
                root.join("data").join("tickets.db").display().to_string(),
            ),
            (
                "OPS_REFERENCE_TIME".to_string(),
                settings.ops_reference_time.clone(),
            ),
        ],
    })
}

pub async fn connect(settings: &McpSettings) -> Result<McpConnection, McpError> {
    McpConnection::start(&launch(settings)?, settings.startup_timeout).await
}
