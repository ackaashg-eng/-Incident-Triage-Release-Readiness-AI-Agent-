//! The real `AnthropicClient` against a local fake Messages API:
//! headers, body, success parsing, retries, error mapping and timeouts.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use agent_client::config::{ApiKey, Effort};
use agent_client::llm::types::{MAX_TOKENS, ToolDefinition};
use agent_client::llm::{AnthropicClient, LlmClient, LlmError, MessagesRequest, StopReason};
use common::{FakeAnthropic, Reply, message};
use serde_json::json;

fn key() -> ApiKey {
    ApiKey::new("sk-ant-test-SECRET123")
}

fn request() -> MessagesRequest {
    MessagesRequest {
        model: "claude-opus-5-5".into(),
        max_tokens: MAX_TOKENS,
        system: "You are a test.".into(),
        tools: vec![ToolDefinition {
            name: "telemetry__search_logs".into(),
            description: "search".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }],
        messages: vec![json!({"role": "user", "content": [{"type": "text", "text": "hi"}]})],
        effort: Effort::Medium,
        fallbacks: true,
        progress_updates: true,
    }
}

fn client(fake: &FakeAnthropic, timeout: Duration) -> AnthropicClient {
    AnthropicClient::new(key(), &fake.url, timeout)
        .unwrap()
        .with_retry_policy(3, Duration::from_millis(10))
}

#[tokio::test]
async fn sends_the_right_headers_and_body_and_parses_the_reply() {
    let fake = FakeAnthropic::start(vec![Reply::json(message(
        "tool_use",
        json!([{"type": "tool_use", "id": "toolu_1", "name": "telemetry__search_logs", "input": {"service_name": "payments-api"}}]),
    ))])
    .await;
    let reply = client(&fake, Duration::from_secs(5))
        .send(&request())
        .await
        .unwrap();
    assert_eq!(reply.stop(), StopReason::ToolUse);
    assert_eq!(
        reply.tool_uses().unwrap()[0].input["service_name"],
        "payments-api"
    );
    assert_eq!(reply.usage.cache_read_input_tokens, Some(900));

    let sent = &fake.requests()[0];
    assert_eq!(sent.header("x-api-key"), Some("sk-ant-test-SECRET123"));
    assert_eq!(sent.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(
        sent.header("anthropic-beta"),
        Some("server-side-fallback-2026-07-01,thinking-display-updates-2026-08-18")
    );
    assert_eq!(sent.header("content-type"), Some("application/json"));
    assert_eq!(sent.body["model"], "claude-opus-5-5");
    assert_eq!(sent.body["tools"][0]["name"], "telemetry__search_logs");
    assert_eq!(sent.body["tool_choice"]["type"], "auto");
    assert!(sent.body.get("temperature").is_none());
}

#[tokio::test]
async fn rate_limits_are_retried_honouring_retry_after() {
    let fake = FakeAnthropic::start(vec![
        Reply::error(429, "rate_limit_error", "slow down").header("retry-after", "0.2"),
        Reply::error(529, "overloaded_error", "busy").header("retry-after", "0"),
        Reply::json(message("end_turn", json!([{"type": "text", "text": "ok"}]))),
    ])
    .await;
    let started = Instant::now();
    let reply = client(&fake, Duration::from_secs(5))
        .send(&request())
        .await
        .unwrap();
    assert_eq!(reply.texts(), ["ok"]);
    assert_eq!(fake.requests().len(), 3);
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "retry-after was honoured"
    );
}

#[tokio::test]
async fn persistent_overload_gives_up_after_the_retries() {
    let fake = FakeAnthropic::start(
        (0..4)
            .map(|_| Reply::error(529, "overloaded_error", "busy"))
            .collect(),
    )
    .await;
    let err = client(&fake, Duration::from_secs(5))
        .send(&request())
        .await
        .unwrap_err();
    assert_eq!(err, LlmError::Overloaded { attempts: 4 });
    assert_eq!(fake.requests().len(), 4, "1 attempt + 3 retries");
}

#[tokio::test]
async fn server_errors_are_retried_then_reported() {
    let fake = FakeAnthropic::start(
        (0..4)
            .map(|_| Reply::error(500, "api_error", "boom"))
            .collect(),
    )
    .await;
    let err = client(&fake, Duration::from_secs(5))
        .send(&request())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, LlmError::Server { status: 500, attempts: 4, message } if message.contains("boom")),
        "{err}"
    );
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    for (reply, check) in [
        (
            Reply::error(401, "authentication_error", "invalid x-api-key"),
            "auth",
        ),
        (Reply::error(403, "permission_error", "nope"), "permission"),
        (
            Reply::error(
                400,
                "invalid_request_error",
                "tools.0.input_schema: invalid",
            ),
            "invalid",
        ),
        (
            Reply::error(404, "not_found_error", "model: claude-nope"),
            "invalid",
        ),
    ] {
        let fake = FakeAnthropic::start(vec![reply]).await;
        let err = client(&fake, Duration::from_secs(5))
            .send(&request())
            .await
            .unwrap_err();
        assert_eq!(fake.requests().len(), 1, "{check}: no retry");
        let ok = match check {
            "auth" => matches!(err, LlmError::Auth(_)),
            "permission" => matches!(err, LlmError::Permission(_)),
            _ => matches!(err, LlmError::InvalidRequest { .. }),
        };
        assert!(ok, "{check}: {err}");
        assert!(
            !err.to_string().contains("SECRET123"),
            "the key never appears in errors"
        );
    }
}

#[tokio::test]
async fn malformed_success_bodies_are_reported() {
    let fake = FakeAnthropic::start(vec![Reply::raw(200, "{not json")]).await;
    let err = client(&fake, Duration::from_secs(5))
        .send(&request())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Malformed(_)), "{err}");
}

#[tokio::test]
async fn slow_responses_time_out_and_are_retried() {
    let slow = || Reply::json(message("end_turn", json!([]))).delay(Duration::from_secs(3));
    let fake = FakeAnthropic::start(vec![slow(), slow()]).await;
    let c = AnthropicClient::new(key(), &fake.url, Duration::from_millis(300))
        .unwrap()
        .with_retry_policy(1, Duration::from_millis(10));
    let started = Instant::now();
    let err = c.send(&request()).await.unwrap_err();
    assert!(
        matches!(err, LlmError::Timeout { attempts: 2, .. }),
        "{err}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "did not wait for the slow server"
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_reported_after_retrying() {
    // Port 9 on localhost: nothing listens there. Windows keeps retrying the
    // TCP connect for ~2 s before giving up, so depending on timing this is a
    // connection error or a timeout. Both are correct, controlled outcomes.
    let c = AnthropicClient::new(key(), "http://127.0.0.1:9", Duration::from_secs(2))
        .unwrap()
        .with_retry_policy(1, Duration::from_millis(10));
    let err = c.send(&request()).await.unwrap_err();
    assert!(
        matches!(
            err,
            LlmError::Network { attempts: 2, .. } | LlmError::Timeout { attempts: 2, .. }
        ),
        "{err}"
    );
}
