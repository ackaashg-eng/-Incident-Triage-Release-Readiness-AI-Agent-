//! Manages the MCP client connections of one agent session.
//!
//! Both servers are started **concurrently** (`tokio::join!`). Each one is
//! identified (name, version, protocol, capabilities) and asked for its
//! tools, resources and resource templates. A server that fails to start or to
//! answer discovery is recorded as *unavailable*; the session continues with
//! the other one (unless none is available).

use std::time::Duration;

use rmcp::model::{Resource, ResourceTemplate};

use super::connection::{McpConnection, McpError};
use super::{filesystem, telemetry};
use crate::agent::registry::DiscoveredServer;
use crate::config::McpSettings;

/// JSON-RPC "method not found": the server simply doesn't offer that list.
const METHOD_NOT_FOUND: i32 = -32601;

pub struct ServerSet {
    connections: Vec<McpConnection>,
    discovered: Vec<DiscoveredServer>,
    unavailable: Vec<(String, String)>,
}

impl ServerSet {
    /// Starts both servers at the same time and discovers what they offer.
    pub async fn connect_all(settings: &McpSettings) -> Self {
        let (telemetry, files) =
            tokio::join!(telemetry::connect(settings), filesystem::connect(settings));
        let mut set = Self {
            connections: Vec::new(),
            discovered: Vec::new(),
            unavailable: Vec::new(),
        };
        for (name, result) in [
            (telemetry::SERVER_NAME, telemetry),
            (filesystem::SERVER_NAME, files),
        ] {
            match result {
                Ok(conn) => set.add(conn, settings.tool_timeout).await,
                Err(err) => {
                    tracing::warn!(server = name, error = %err, "MCP server unavailable");
                    set.unavailable.push((name.to_string(), err.to_string()));
                }
            }
        }
        set
    }

    async fn add(&mut self, conn: McpConnection, timeout: Duration) {
        match discover(&conn, timeout).await {
            Ok(found) => {
                tracing::info!(
                    server = found.name,
                    tools = found.tools.len(),
                    resources = found.resources.len(),
                    templates = found.templates.len(),
                    "discovery complete"
                );
                self.discovered.push(found);
                self.connections.push(conn);
            }
            Err(err) => {
                self.unavailable
                    .push((conn.name().to_string(), format!("discovery failed: {err}")));
                let _ = conn.shutdown().await;
            }
        }
    }

    pub fn get(&self, server: &str) -> Option<&McpConnection> {
        self.connections.iter().find(|c| c.name() == server)
    }

    pub fn discovered(&self) -> &[DiscoveredServer] {
        &self.discovered
    }

    /// `(server, reason)` for servers that could not be used.
    pub fn unavailable(&self) -> &[(String, String)] {
        &self.unavailable
    }

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Stops every server (in parallel), logging but not failing on errors.
    pub async fn shutdown(self) {
        let stops = self.connections.into_iter().map(|c| async move {
            let name = c.name();
            if let Err(err) = c.shutdown().await {
                tracing::warn!(server = name, error = %err, "MCP server did not stop cleanly");
            }
        });
        for stop in stops {
            stop.await;
        }
    }
}

async fn discover(conn: &McpConnection, timeout: Duration) -> Result<DiscoveredServer, McpError> {
    let tools = conn.list_tools(timeout).await?;
    let resources: Vec<Resource> = optional(conn.list_resources(timeout).await)?;
    let templates: Vec<ResourceTemplate> = optional(conn.list_resource_templates(timeout).await)?;
    let caps = conn.capabilities();
    Ok(DiscoveredServer {
        name: conn.name(),
        label: format!(
            "{}, protocol {}, capabilities: {}",
            conn.server_label(),
            conn.protocol_version(),
            if caps.is_empty() {
                "none".to_string()
            } else {
                caps.join(", ")
            }
        ),
        tools,
        resources,
        templates,
    })
}

/// A list the server doesn't implement is simply empty.
fn optional<T>(result: Result<Vec<T>, McpError>) -> Result<Vec<T>, McpError> {
    match result {
        Err(McpError::Rejected {
            code: METHOD_NOT_FOUND,
            ..
        }) => Ok(Vec::new()),
        other => other,
    }
}
