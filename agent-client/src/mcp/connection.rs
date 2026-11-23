//! One MCP client connection to one server running as a child process (stdio).
//!
//! Responsibilities:
//! * launch the server with an explicit program, argument list, working
//!   directory and a scrubbed environment (no shell, so paths with spaces are safe);
//! * complete the MCP handshake within a startup timeout;
//! * run every request under a timeout, so a hung server can't hang the agent;
//! * turn SDK errors into a small [`McpError`] the agent can act on
//!   (especially *disconnected* vs. *the server rejected the request*);
//! * keep the last lines the server wrote to stderr, for diagnostics;
//! * shut the server down (dropping a connection also kills the process).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, JsonObject, ReadResourceRequestParams,
    ReadResourceResult, Resource, ResourceTemplate, Tool,
};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::{ServiceError, ServiceExt};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::JoinHandle;

use super::child_env;

const STDERR_TAIL_LINES: usize = 20;

/// How to start one server process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLaunch {
    /// Short name used in logs, errors and (later) tool prefixes, e.g. `files`.
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Extra variables on top of the scrubbed base environment.
    pub env: Vec<(String, String)>,
}

impl ServerLaunch {
    /// The command line as one readable string (for logs and the check report).
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(OsString::as_os_str))
            .map(|a| {
                let s = a.to_string_lossy();
                if s.contains(' ') {
                    format!("\"{s}\"")
                } else {
                    s.into_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error("[{server}] could not start `{program}`: {reason}")]
    Spawn {
        server: &'static str,
        program: String,
        reason: String,
    },

    #[error(
        "[{server}] did not finish the MCP handshake within {secs}s{}",
        tail(stderr)
    )]
    StartupTimeout {
        server: &'static str,
        secs: u64,
        stderr: Vec<String>,
    },

    #[error("[{server}] MCP handshake failed: {reason}{}", tail(stderr))]
    Handshake {
        server: &'static str,
        reason: String,
        stderr: Vec<String>,
    },

    #[error("[{server}] {operation} timed out after {secs}s")]
    Timeout {
        server: &'static str,
        operation: String,
        secs: u64,
    },

    #[error("[{server}] the server process has disconnected{}", tail(stderr))]
    Disconnected {
        server: &'static str,
        stderr: Vec<String>,
    },

    /// The server answered with a JSON-RPC error (e.g. access denied, not found).
    #[error("[{server}] {operation} was rejected ({code}): {message}")]
    Rejected {
        server: &'static str,
        operation: String,
        code: i32,
        message: String,
        data: Option<Value>,
    },

    #[error("[{server}] {operation} failed: {reason}")]
    Request {
        server: &'static str,
        operation: String,
        reason: String,
    },

    /// The launch configuration or the server itself violates a safety rule.
    #[error("[{server}] refused for safety: {reason}")]
    Unsafe {
        server: &'static str,
        reason: String,
    },
}

fn tail(stderr: &[String]) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(". Server stderr (last lines): {}", stderr.join(" | "))
    }
}

type Tail = Arc<Mutex<VecDeque<String>>>;

/// A live connection. Dropping it closes stdin and kills the child process.
pub struct McpConnection {
    name: &'static str,
    service: RunningService<RoleClient, ()>,
    pid: Option<u32>,
    stderr_tail: Tail,
    stderr_task: Option<JoinHandle<()>>,
}

impl McpConnection {
    /// Launches the server and completes the MCP handshake.
    pub async fn start(launch: &ServerLaunch, startup_timeout: Duration) -> Result<Self, McpError> {
        let server = launch.name;
        let mut cmd = tokio::process::Command::new(&launch.program);
        cmd.args(&launch.args)
            .current_dir(&launch.cwd)
            .env_clear()
            .envs(child_env::inherited())
            .envs(launch.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));

        tracing::info!(server, command = %launch.display(), "starting MCP server");
        let (transport, stderr) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| McpError::Spawn {
                server,
                program: launch.program.display().to_string(),
                reason: if e.kind() == std::io::ErrorKind::NotFound {
                    "program not found (is it installed and on PATH?)".to_string()
                } else {
                    e.to_string()
                },
            })?;
        let pid = transport.id();

        let stderr_tail: Tail = Arc::default();
        let stderr_task = stderr.map(|stderr| {
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(server, "stderr: {line}");
                    if let Ok(mut t) = tail.lock() {
                        if t.len() == STDERR_TAIL_LINES {
                            t.pop_front();
                        }
                        t.push_back(line);
                    }
                }
            })
        });

        let handshake = tokio::time::timeout(startup_timeout, ().serve(transport)).await;
        let service = match handshake {
            Ok(Ok(service)) => service,
            // In both failure cases the transport has been dropped, which kills the child.
            Ok(Err(err)) => {
                let stderr = drain_tail(stderr_task, &stderr_tail).await;
                return Err(McpError::Handshake {
                    server,
                    reason: err.to_string(),
                    stderr,
                });
            }
            Err(_) => {
                let stderr = drain_tail(stderr_task, &stderr_tail).await;
                return Err(McpError::StartupTimeout {
                    server,
                    secs: startup_timeout.as_secs(),
                    stderr,
                });
            }
        };

        let conn = Self {
            name: server,
            service,
            pid,
            stderr_tail,
            stderr_task,
        };
        tracing::info!(
            server,
            pid = ?conn.pid,
            remote = %conn.server_label(),
            protocol = %conn.protocol_version(),
            "MCP server connected"
        );
        Ok(conn)
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// `name version` as reported by the server.
    pub fn server_label(&self) -> String {
        self.service
            .peer_info()
            .and_then(|info| info.server_info.clone())
            .map_or_else(
                || "(unnamed server)".to_string(),
                |i| format!("{} {}", i.name, i.version),
            )
    }

    pub fn protocol_version(&self) -> String {
        self.service.peer_info().map_or_else(
            || "unknown".to_string(),
            |info| info.protocol_version.to_string(),
        )
    }

    pub fn is_closed(&self) -> bool {
        self.service.is_closed()
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        snapshot(&self.stderr_tail)
    }

    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>, McpError> {
        self.request("tools/list", timeout, self.service.list_all_tools())
            .await
    }

    pub async fn list_resources(&self, timeout: Duration) -> Result<Vec<Resource>, McpError> {
        self.request("resources/list", timeout, self.service.list_all_resources())
            .await
    }

    pub async fn list_resource_templates(
        &self,
        timeout: Duration,
    ) -> Result<Vec<ResourceTemplate>, McpError> {
        self.request(
            "resources/templates/list",
            timeout,
            self.service.list_all_resource_templates(),
        )
        .await
    }

    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: JsonObject,
        timeout: Duration,
    ) -> Result<CallToolResult, McpError> {
        let params = CallToolRequestParams::new(tool.to_string()).with_arguments(arguments);
        self.request(
            &format!("tools/call {tool}"),
            timeout,
            self.service.call_tool(params),
        )
        .await
    }

    pub async fn read_resource(
        &self,
        uri: &str,
        timeout: Duration,
    ) -> Result<ReadResourceResult, McpError> {
        let params = ReadResourceRequestParams::new(uri);
        self.request(
            &format!("resources/read {uri}"),
            timeout,
            self.service.read_resource(params),
        )
        .await
    }

    /// Closes the connection and waits (bounded) for the process to exit.
    pub async fn shutdown(self) -> Result<(), McpError> {
        let server = self.name;
        let stderr_task = self.stderr_task;
        match tokio::time::timeout(Duration::from_secs(5), self.service.cancel()).await {
            Ok(Ok(reason)) => tracing::info!(server, ?reason, "MCP server stopped"),
            Ok(Err(err)) => {
                return Err(McpError::Request {
                    server,
                    operation: "shutdown".to_string(),
                    reason: err.to_string(),
                });
            }
            Err(_) => {
                return Err(McpError::Timeout {
                    server,
                    operation: "shutdown".to_string(),
                    secs: 5,
                });
            }
        }
        if let Some(task) = stderr_task {
            // The pipe closes when the process exits; don't wait forever.
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
        Ok(())
    }

    async fn request<T>(
        &self,
        operation: &str,
        timeout: Duration,
        fut: impl Future<Output = Result<T, ServiceError>>,
    ) -> Result<T, McpError> {
        let server = self.name;
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(value)) => Ok(value),
            Err(_) => Err(McpError::Timeout {
                server,
                operation: operation.to_string(),
                secs: timeout.as_secs(),
            }),
            Ok(Err(ServiceError::McpError(data))) => Err(McpError::Rejected {
                server,
                operation: operation.to_string(),
                code: data.code.0,
                message: data.message.to_string(),
                data: data.data,
            }),
            Ok(Err(
                ServiceError::TransportClosed
                | ServiceError::TransportSend(_)
                | ServiceError::Cancelled { .. },
            )) => Err(McpError::Disconnected {
                server,
                stderr: self.stderr_tail(),
            }),
            Ok(Err(other)) => {
                if self.is_closed() {
                    Err(McpError::Disconnected {
                        server,
                        stderr: self.stderr_tail(),
                    })
                } else {
                    Err(McpError::Request {
                        server,
                        operation: operation.to_string(),
                        reason: other.to_string(),
                    })
                }
            }
        }
    }
}

fn snapshot(tail: &Tail) -> Vec<String> {
    tail.lock()
        .map(|t| t.iter().cloned().collect())
        .unwrap_or_default()
}

/// After a failed start, give the stderr reader a moment to collect the
/// process's last words (it ends when the killed process closes the pipe).
async fn drain_tail(task: Option<JoinHandle<()>>, tail: &Tail) -> Vec<String> {
    if let Some(task) = task {
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    }
    snapshot(tail)
}
