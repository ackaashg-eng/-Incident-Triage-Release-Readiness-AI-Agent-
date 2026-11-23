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
