//! `AnthropicClient` against a local fake endpoint (no network, no API key).
//! Hand-rolled HTTP server on tokio only, so it needs no extra dev-dependencies.

// Test helpers outside `#[test]` fns are not covered by clippy.toml's allow-unwrap-in-tests.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client::config::{ApiKey, Effort};
use agent_client::llm::types::{API_VERSION, MAX_TOKENS, ToolDefinition};
use agent_client::llm::{AnthropicClient, LlmClient, LlmError, MessagesRequest, StopReason};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Reply {
    status: u16,
    body: String,
    headers: Vec<(&'static str, &'static str)>,
}

fn ok(body: Value) -> Reply {
    Reply {
        status: 200,
        body: body.to_string(),
        headers: vec![],
    }
}
fn status(status: u16, body: Value) -> Reply {
    Reply {
        status,
        body: body.to_string(),
        headers: vec![],
    }
}

/// What the fake server saw for one request: the raw header block and the JSON body.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// Serves `replies` in order (the last one repeats).
async fn fake(replies: Vec<Reply>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        let mut n = 0usize;
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let (head_end, len) = loop {
                let r = sock.read(&mut tmp).await.unwrap();
                if r == 0 {
                    break (0, 0);
                }
                buf.extend_from_slice(&tmp[..r]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (p + 4, len);
                }
            };
            while buf.len() < head_end + len {
                let r = sock.read(&mut tmp).await.unwrap();
                if r == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..r]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
            let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
            log.lock().unwrap().push((head, body));
            let reply = &replies[n.min(replies.len() - 1)];
            n += 1;
            let mut resp = format!(
                "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                reply.status,
                reply.body.len()
            );
            for (k, v) in &reply.headers {
                resp.push_str(&format!("{k}: {v}\r\n"));
            }
            resp.push_str("\r\n");
            resp.push_str(&reply.body);
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (url, seen)
}

fn client(url: &str) -> AnthropicClient {
    AnthropicClient::new(ApiKey::new("sk-ant-TESTKEY"), url, Duration::from_secs(5))
        .unwrap()
        .with_retry_policy(2, Duration::from_millis(5))
}

fn request() -> MessagesRequest {
    MessagesRequest {
        model: "claude-opus-5-5".into(),
        max_tokens: MAX_TOKENS,
        system: "sys".into(),
        tools: vec![ToolDefinition {
            name: "telemetry__search_logs".into(),
            description: "d".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }],
        messages: vec![json!({"role": "user", "content": "hi"})],
        effort: Effort::Medium,
        fallbacks: true,
        progress_updates: true,
    }
}

fn message() -> Value {
    json!({"id": "msg_1", "model": "claude-opus-5-5", "stop_reason": "end_turn",
        "content": [{"type": "text", "text": "done"}],
        "usage": {"input_tokens": 3, "output_tokens": 2}})
}

fn error_body(kind: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"type": kind, "message": message}})
}

#[tokio::test]
async fn a_successful_call_sends_the_required_headers_and_body() {
    let (url, seen) = fake(vec![ok(message())]).await;
    let resp = client(&url).send(&request()).await.unwrap();
    assert_eq!(resp.stop(), StopReason::EndTurn);
    assert_eq!(resp.texts(), ["done"]);
    assert_eq!(resp.usage.output_tokens, 2);

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (head, body) = &seen[0];
    assert!(head.starts_with("post /v1/messages "), "{head}");
    assert!(head.contains("x-api-key: sk-ant-testkey"));
    assert!(head.contains(&format!(
        "anthropic-version: {}",
        API_VERSION.to_lowercase()
    )));
    assert!(head.contains("anthropic-beta: server-side-fallback"));
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["tool_choice"], json!({"type": "auto"}));
}

#[tokio::test]
async fn rate_limit_then_success_is_retried_and_retry_after_is_honoured() {
    let (url, seen) = fake(vec![
        Reply {
            status: 429,
            body: error_body("rate_limit_error", "slow down").to_string(),
            headers: vec![("retry-after", "0")],
        },
        ok(message()),
    ])
    .await;
    let resp = client(&url).send(&request()).await.unwrap();
    assert_eq!(resp.stop(), StopReason::EndTurn);
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn overload_529_and_server_errors_give_up_after_the_retry_budget() {
    let (url, seen) = fake(vec![status(529, error_body("overloaded_error", "busy"))]).await;
    let err = client(&url).send(&request()).await.unwrap_err();
    assert!(
        matches!(err, LlmError::Overloaded { attempts: 3 }),
        "{err:?}"
    );
    assert_eq!(seen.lock().unwrap().len(), 3);

    let (url, seen) = fake(vec![status(500, error_body("api_error", "boom"))]).await;
    let err = client(&url).send(&request()).await.unwrap_err();
    assert!(
        matches!(
            err,
            LlmError::Server {
                status: 500,
                attempts: 3,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn persistent_rate_limiting_is_reported_as_rate_limited() {
    let (url, _) = fake(vec![status(
        429,
        error_body("rate_limit_error", "slow down"),
    )])
    .await;
    let err = client(&url).send(&request()).await.unwrap_err();
    assert!(
        matches!(err, LlmError::RateLimited { attempts: 3, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    let cases = [
        (401, "authentication_error"),
        (403, "permission_error"),
        (400, "invalid_request_error"),
        (404, "not_found_error"),
        (413, "request_too_large"),
    ];
    for (code, kind) in cases {
        let (url, seen) = fake(vec![status(code, error_body(kind, "nope"))]).await;
        let err = client(&url).send(&request()).await.unwrap_err();
        match (code, &err) {
            (401, LlmError::Auth(m)) => assert!(m.contains("nope")),
            (403, LlmError::Permission(m)) => assert!(m.contains("nope")),
            (_, LlmError::InvalidRequest { status, message }) => {
                assert_eq!(*status, code);
                assert!(message.contains(kind));
            }
            other => panic!("{code}: unexpected {other:?}"),
        }
        assert_eq!(seen.lock().unwrap().len(), 1, "{code} must not be retried");
    }
}

#[tokio::test]
async fn an_unreadable_success_body_is_malformed() {
    let (url, _) = fake(vec![Reply {
        status: 200,
        body: "not json".into(),
        headers: vec![],
    }])
    .await;
    let err = client(&url).send(&request()).await.unwrap_err();
    assert!(matches!(err, LlmError::Malformed(_)), "{err:?}");
}

#[tokio::test]
async fn a_refusal_carries_its_category() {
    let body = json!({"id": "m", "model": "x", "content": [], "stop_reason": "refusal",
        "stop_details": {"category": "cyber"}, "usage": {"input_tokens": 1, "output_tokens": 0}});
    let (url, _) = fake(vec![ok(body)]).await;
    let resp = client(&url).send(&request()).await.unwrap();
    assert_eq!(resp.stop(), StopReason::Refusal(Some("cyber".into())));
}

#[tokio::test]
async fn an_unreachable_server_is_a_network_error_and_the_key_never_leaks() {
    // Nothing listens on this port.
    let err = client("http://127.0.0.1:1")
        .send(&request())
        .await
        .unwrap_err();
    assert!(
        matches!(err, LlmError::Network { attempts: 3, .. }),
        "{err:?}"
    );
    assert!(!format!("{err:?} {err}").contains("TESTKEY"));
}
