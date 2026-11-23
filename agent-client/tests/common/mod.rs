//! Shared test helpers.
//!
//! `FakeAnthropic` is a tiny local HTTP server that imitates `POST /v1/messages`:
//! it answers with scripted replies (in order) and records every request
//! (headers + JSON body). It lets us test the real HTTP client and the real
//! binary without an API key and without network access.

#![allow(dead_code, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn json(body: Value) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    pub fn error(status: u16, kind: &str, message: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: json!({"type": "error", "error": {"type": kind, "message": message}}).to_string(),
            delay: Duration::ZERO,
        }
    }

    pub fn raw(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn delay(mut self, d: Duration) -> Self {
        self.delay = d;
        self
    }
}

/// A Messages API response body.
pub fn message(stop_reason: &str, content: Value) -> Value {
    json!({
        "id": "msg_test", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": content, "stop_reason": stop_reason,
        "usage": {"input_tokens": 1200, "output_tokens": 80, "cache_read_input_tokens": 900}
    })
}

#[derive(Debug, Clone)]
pub struct Recorded {
    /// Lower-case header names.
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeAnthropic {
    pub url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeAnthropic {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let replies = Arc::new(Mutex::new(
            replies
                .into_iter()
                .collect::<std::collections::VecDeque<_>>(),
        ));
        let recorded = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let recorded = Arc::clone(&recorded);
                let replies = Arc::clone(&replies);
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    recorded.lock().unwrap().push(request);
                    let reply = replies.lock().unwrap().pop_front().unwrap_or_else(|| {
                        Reply::error(500, "api_error", "no more scripted replies")
                    });
                    tokio::time::sleep(reply.delay).await;
                    let mut head = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    for (k, v) in &reply.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(reply.body.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { url, requests }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let headers: Vec<(String, String)> = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < header_end + length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&buf[header_end..]).unwrap_or(Value::Null);
    Some(Recorded { headers, body })
}

/// The project folder (for tests that run the real servers).
pub fn project_root() -> std::path::PathBuf {
    agent_client::mcp::filesystem::canonical(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."),
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// Scripted Claude + agent builder (agent-level tests with the REAL servers)
// ---------------------------------------------------------------------------

use agent_client::agent::approval::Approver;
use agent_client::agent::{Agent, AgentEvent, AgentSettings, Observer, ToolCallRecord, ToolStatus};
use agent_client::config::{Effort, McpSettings};
use agent_client::llm::{LlmClient, LlmError, MessageResponse, MessagesRequest};
use agent_client::mcp::manager::ServerSet;

/// Replays fixed Claude responses and records every request.
#[derive(Clone, Default)]
pub struct ScriptedLlm {
    replies: Arc<Mutex<std::collections::VecDeque<Result<MessageResponse, LlmError>>>>,
    requests: Arc<Mutex<Vec<MessagesRequest>>>,
}

impl ScriptedLlm {
    pub fn new(replies: Vec<Result<Value, LlmError>>) -> Self {
        let replies = replies
            .into_iter()
            .map(|r| r.map(|v| serde_json::from_value(v).unwrap()))
            .collect();
        Self {
            replies: Arc::new(Mutex::new(replies)),
            requests: Arc::default(),
        }
    }

    pub fn requests(&self) -> Vec<MessagesRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl LlmClient for ScriptedLlm {
    async fn send(&self, request: &MessagesRequest) -> Result<MessageResponse, LlmError> {
        self.requests.lock().unwrap().push(request.clone());
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(LlmError::Malformed("script ended".into())))
    }
}

/// Records events as short strings (`.0`) and full tool records (`.1`).
#[derive(Default)]
pub struct Recorder(pub Mutex<Vec<String>>, pub Mutex<Vec<ToolCallRecord>>);

impl Recorder {
    pub fn records(&self) -> Vec<ToolCallRecord> {
        self.1.lock().unwrap().clone()
    }
}

impl Observer for Recorder {
    fn on_event(&self, event: &AgentEvent<'_>) {
        let line = match event {
            AgentEvent::ToolStart { name, .. } => format!("start {name}"),
            AgentEvent::ToolEnd(rec) => {
                self.1.lock().unwrap().push((*rec).clone());
                format!("end {} {}", rec.name, rec.status == ToolStatus::Ok)
            }
            AgentEvent::Approval { proposal, .. } => format!("proposal {}", proposal.tool),
            AgentEvent::Note(n) => format!("note {n}"),
            AgentEvent::Warning(w) => format!("warning {w}"),
            _ => return,
        };
        self.0.lock().unwrap().push(line);
    }
}

pub fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

/// MCP settings for the real servers, with a throw-away ticket DB.
pub fn mcp_settings(tickets: &std::path::Path) -> McpSettings {
    let mut s = McpSettings::from_lookup(&project_root(), &|_: &str| None).unwrap();
    s.tickets_db = tickets.to_path_buf();
    s
}

pub fn agent_settings(max_iterations: u32) -> AgentSettings {
    AgentSettings {
        model: "claude-opus-5-5".into(),
        effort: Effort::Medium,
        fallbacks: true,
        progress_updates: true,
        max_iterations,
        max_tokens: 16_000,
        tool_timeout: Duration::from_secs(10),
        tool_timeouts: Default::default(),
        secrets: vec!["sk-ant-test-SECRET-KEY-0123456789".to_string()],
        path_aliases: {
            let mut a = vec![(
                project_root().display().to_string(),
                "<project>".to_string(),
            )];
            if let Some(home) = std::env::var_os("USERPROFILE") {
                a.push((home.to_string_lossy().into_owned(), "<home>".to_string()));
            }
            a
        },
    }
}

pub async fn build_agent(
    llm: ScriptedLlm,
    tickets: &std::path::Path,
    max_iterations: u32,
    approver: Box<dyn Approver>,
) -> Agent<ScriptedLlm> {
    let servers = ServerSet::connect_all(&mcp_settings(tickets)).await;
    assert!(
        servers.unavailable().is_empty(),
        "{:?}",
        servers.unavailable()
    );
    Agent::new(
        llm,
        agent_settings(max_iterations),
        servers,
        "2026-11-20T12:00:00Z",
        approver,
    )
}

/// The tool_result blocks of a request's last message: (id, content, is_error).
pub fn tool_results(request: &MessagesRequest) -> Vec<(String, String, bool)> {
    let last = request.messages.last().unwrap();
    assert_eq!(last["role"], "user");
    last["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["tool_use_id"].as_str().unwrap().to_string(),
                b["content"].as_str().unwrap().to_string(),
                b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            )
        })
        .collect()
}
