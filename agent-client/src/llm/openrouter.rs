//! HTTP client for OpenRouter's `POST /chat/completions` (OpenAI-compatible).
//!
//! The agent keeps speaking "Anthropic shape" internally: `MessagesRequest::to_body()`
//! produces an Anthropic Messages body and the history holds raw Anthropic content
//! blocks (`text`, `tool_use`, `tool_result`). This client translates at the HTTP
//! boundary only:
//!
//! * request : Anthropic Messages body  -> OpenAI chat body (whitelisted fields only,
//!   so `thinking`, `output_config`, `fallbacks` and the beta header never leave);
//! * response: OpenAI chat response     -> Anthropic-shaped `MessageResponse`.
//!
//! So `runner.rs`, history handling, approvals and rollback do not change.
//!
//! * Retries (up to 3): 429, 408, 5xx, upstream `finish_reason: "error"`, network
//!   errors and timeouts. `retry-after` is honoured (capped). A *daily* free-model
//!   limit is not retried: it cannot fix itself within seconds.
//! * The API key only goes into the `Authorization` header, never into an error.
//! * Thinking blocks are not replayed (OpenRouter has no equivalent).

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};

use super::anthropic::{LlmClient, LlmError};
use super::types::{MessageResponse, MessagesRequest};
use crate::config::ApiKey;

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub struct OpenRouterClient {
    http: reqwest::Client,
    api_key: ApiKey,
    url: String,
    model: String,
    fallbacks: Vec<String>,
    deny_data_collection: bool,
    timeout: Duration,
    max_retries: u32,
    base_delay: Duration,
}

impl OpenRouterClient {
    pub fn new(
        api_key: ApiKey,
        model: String,
        base_url: Option<String>,
        fallbacks: Vec<String>,
        timeout: Duration,
    ) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("incident-triage-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| LlmError::Network {
                attempts: 0,
                message: e.to_string(),
            })?;
        let base = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Ok(Self {
            http,
            api_key,
            url: format!("{}/chat/completions", base.trim_end_matches('/')),
            model,
            fallbacks,
            deny_data_collection: false,
            timeout,
            max_retries: 3,
            base_delay: Duration::from_secs(1),
        })
    }

    /// Tests use small values so retries are fast.
    pub fn with_retry_policy(mut self, max_retries: u32, base_delay: Duration) -> Self {
        self.max_retries = max_retries;
        self.base_delay = base_delay;
        self
    }

    /// Only route to providers that do not store/train on prompts. Off by default
    /// because most `:free` models are served by providers that do, so turning it on
    /// makes those models unreachable ("no endpoints found").
    pub fn with_data_collection_denied(mut self, deny: bool) -> Self {
        self.deny_data_collection = deny;
        self
    }

    async fn send_once(&self, body: &Value) -> Attempt {
        let response = match self
            .http
            .post(&self.url)
            .bearer_auth(self.api_key.expose())
            .header("X-Title", "incident-triage-agent")
            .json(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Attempt::Retry(Failure::Timeout, None),
            Err(e) => return Attempt::Retry(Failure::Network(describe(e)), None),
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| Duration::from_secs_f64(s).min(MAX_RETRY_AFTER));
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) if e.is_timeout() => return Attempt::Retry(Failure::Timeout, None),
            Err(e) => return Attempt::Retry(Failure::Network(describe(e)), None),
        };
        let parsed: Option<Value> = serde_json::from_slice(&bytes).ok();

        if status.is_success() {
            let Some(json) = parsed else {
                return Attempt::Done(Err(LlmError::Malformed(
                    "the response was not JSON".to_string(),
                )));
            };
            // OpenRouter can answer HTTP 200 with a top-level `error` object.
            if json.get("error").is_some() {
                let code = json
                    .pointer("/error/code")
                    .and_then(Value::as_u64)
                    .and_then(|c| u16::try_from(c).ok())
                    .unwrap_or(502);
                return classify_error(code, error_message(&json), retry_after);
            }
            return match from_openrouter_response(&json, &self.model) {
                Ok(anthropic_shape) => {
                    match serde_json::from_value::<MessageResponse>(anthropic_shape) {
                        Ok(msg) => {
                            tracing::debug!(id = %msg.id, stop = ?msg.stop_reason, "OpenRouter response");
                            Attempt::Done(Ok(msg))
                        }
                        Err(e) => Attempt::Done(Err(LlmError::Malformed(e.to_string()))),
                    }
                }
                Err(Problem::Upstream(message)) => {
                    Attempt::Retry(Failure::Server(502, message), retry_after)
                }
                Err(Problem::Malformed(message)) => {
                    Attempt::Done(Err(LlmError::Malformed(message)))
                }
            };
        }

        let message = match &parsed {
            Some(v) => error_message(v),
            None => String::from_utf8_lossy(&bytes).chars().take(300).collect(),
        };
        tracing::warn!(status = status.as_u16(), %message, "OpenRouter API error");
        classify_error(status.as_u16(), message, retry_after)
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let exp = self
            .base_delay
            .saturating_mul(2u32.saturating_pow(attempt))
            .min(MAX_BACKOFF);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        exp + exp.mul_f64(f64::from(nanos % 1000) / 4000.0)
    }
}

impl LlmClient for OpenRouterClient {
    async fn send(&self, request: &MessagesRequest) -> Result<MessageResponse, LlmError> {
        let body = to_openrouter_body(
            &request.to_body(),
            &self.model,
            &self.fallbacks,
            self.deny_data_collection,
        );
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (failure, retry_after) = match self.send_once(&body).await {
                Attempt::Done(result) => return result,
                Attempt::Retry(failure, retry_after) => (failure, retry_after),
            };
            if attempt > self.max_retries {
                return Err(failure.into_error(attempt, self.timeout));
            }
            let delay = retry_after.unwrap_or_else(|| self.backoff(attempt - 1));
            tracing::warn!(attempt, ?delay, failure = ?failure, "retrying the OpenRouter request");
            tokio::time::sleep(delay).await;
        }
    }
}

enum Attempt {
    Done(Result<MessageResponse, LlmError>),
    Retry(Failure, Option<Duration>),
}

#[derive(Debug)]
enum Failure {
    Timeout,
    Network(String),
    RateLimited(String),
    Server(u16, String),
}

impl Failure {
    fn into_error(self, attempts: u32, timeout: Duration) -> LlmError {
        match self {
            Self::Timeout => LlmError::Timeout {
                secs: timeout.as_secs(),
                attempts,
            },
            Self::Network(message) => LlmError::Network { attempts, message },
            Self::RateLimited(message) => LlmError::RateLimited { attempts, message },
            Self::Server(status, message) => LlmError::Server {
                status,
                attempts,
                message,
            },
        }
    }
}

/// Maps an HTTP status (real or from a 200-with-error body) to retry / final error.
fn classify_error(status: u16, message: String, retry_after: Option<Duration>) -> Attempt {
    match StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY) {
        StatusCode::UNAUTHORIZED => Attempt::Done(Err(LlmError::Auth(message))),
        StatusCode::FORBIDDEN => Attempt::Done(Err(LlmError::Permission(message))),
        StatusCode::TOO_MANY_REQUESTS => {
            // The free-tier DAILY cap will not recover in seconds: fail fast and say so.
            if message.contains("per-day") || message.contains("per day") {
                Attempt::Done(Err(LlmError::RateLimited {
                    attempts: 1,
                    message: format!(
                        "{message} (daily free-model limit reached; wait until it resets or add credits)"
                    ),
                }))
            } else {
                Attempt::Retry(Failure::RateLimited(message), retry_after)
            }
        }
        StatusCode::REQUEST_TIMEOUT => Attempt::Retry(Failure::Timeout, retry_after),
        s if s.is_server_error() => {
            Attempt::Retry(Failure::Server(s.as_u16(), message), retry_after)
        }
        s => Attempt::Done(Err(LlmError::InvalidRequest {
            status: s.as_u16(),
            message,
        })),
    }
}

/// `{"error":{"code":..,"message":"..","metadata":{"provider_name":".."}}}` -> text.
fn error_message(v: &Value) -> String {
    let msg = v
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    let text = match v
        .pointer("/error/metadata/provider_name")
        .and_then(Value::as_str)
    {
        Some(p) => format!("{msg} (provider {p})"),
        None => msg.to_string(),
    };
    text.chars().take(500).collect()
}

/// The error and its causes on one line, without the URL.
fn describe(err: reqwest::Error) -> String {
    let err = err.without_url();
    let mut parts = vec![err.to_string()];
    let mut source = std::error::Error::source(&err);
    while let Some(s) = source {
        parts.push(s.to_string());
        source = s.source();
    }
    parts.dedup();
    parts.join(": ")
}

// ---------------------------------------------------------------- request ----

/// Whitelist translation: only fields OpenRouter understands are forwarded.
pub fn to_openrouter_body(
    body: &Value,
    model: &str,
    fallbacks: &[String],
    deny_data_collection: bool,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = system_text(body.get("system")) {
        messages.push(json!({"role": "system", "content": system}));
    }
    for m in body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        convert_message(m, &mut messages);
    }

    let tools: Vec<Value> = body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.get("name").cloned().unwrap_or(Value::Null),
                    "description": t.get("description").cloned().unwrap_or(json!("")),
                    "parameters": t.get("input_schema").cloned()
                        .unwrap_or(json!({"type": "object", "properties": {}})),
                }
            })
        })
        .collect();

    let mut provider = json!({"require_parameters": true});
    if deny_data_collection {
        provider["data_collection"] = json!("deny");
    }
    let mut out = json!({
        "model": model,
        "messages": messages,
        "max_tokens": body.get("max_tokens").cloned().unwrap_or(json!(4096)),
        "provider": provider,
    });
    if !tools.is_empty() {
        out["tools"] = Value::Array(tools);
        out["tool_choice"] = json!("auto");
    }
    if !fallbacks.is_empty() {
        let mut all = vec![model.to_string()];
        all.extend(fallbacks.iter().cloned());
        out["models"] = json!(all);
    }
    out
}

fn system_text(system: Option<&Value>) -> Option<String> {
    match system? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(blocks) => {
            let t: Vec<&str> = blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            (!t.is_empty()).then(|| t.join("\n\n"))
        }
        _ => None,
    }
}

fn convert_message(m: &Value, out: &mut Vec<Value>) {
    let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
    let content = m.get("content");

    if let Some(Value::String(s)) = content {
        out.push(json!({"role": role, "content": s}));
        return;
    }
    let blocks = content
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if role == "assistant" {
        let mut text = String::new();
        let mut calls: Vec<Value> = Vec::new();
        for b in &blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        text.push_str(t);
                    }
                }
                Some("tool_use") => calls.push(json!({
                    "id": b.get("id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": b.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": b.get("input").cloned().unwrap_or(json!({})).to_string(),
                    }
                })),
                _ => {} // thinking / fallback / other Anthropic-only blocks: not replayed
            }
        }
        let mut msg = json!({"role": "assistant", "content": text});
        if !calls.is_empty() {
            if text.is_empty() {
                msg["content"] = Value::Null;
            }
            msg["tool_calls"] = Value::Array(calls);
        }
        out.push(msg);
        return;
    }

    // user message: each tool_result becomes its own `role: "tool"` message (they must
    // directly follow the assistant's tool_calls); any plain text follows them.
    let mut text = String::new();
    for b in &blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("tool_result") => {
                let mut c = tool_result_text(b.get("content"));
                if b.get("is_error").and_then(Value::as_bool) == Some(true) {
                    c = format!("[tool error] {c}");
                }
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(Value::Null),
                    "content": c,
                }));
            }
            Some("text") => {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        out.push(json!({"role": "user", "content": text}));
    }
}

fn tool_result_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// --------------------------------------------------------------- response ----

enum Problem {
    /// The upstream provider failed (retryable).
    Upstream(String),
    Malformed(String),
}

/// Builds an Anthropic-shaped message JSON (deserialisable as `MessageResponse`).
fn from_openrouter_response(resp: &Value, model: &str) -> Result<Value, Problem> {
    let choice = resp
        .pointer("/choices/0")
        .ok_or_else(|| Problem::Malformed("the response has no choices".into()))?;
    let msg = choice.get("message").cloned().unwrap_or(Value::Null);

    let mut content: Vec<Value> = Vec::new();
    if let Some(t) = msg.get("content").and_then(Value::as_str)
        && !t.is_empty()
    {
        content.push(json!({"type": "text", "text": t}));
    }
    let calls = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (i, c) in calls.iter().enumerate() {
        let raw = c
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        // Malformed JSON must not crash the loop: surface it so schema validation
        // rejects it and the model can retry with valid arguments.
        let candidate = if raw.trim().is_empty() { "{}" } else { raw };
        let input = match serde_json::from_str::<Value>(candidate) {
            Ok(v) if v.is_object() => v,
            _ => json!({"_invalid_arguments": raw}),
        };
        content.push(json!({
            "type": "tool_use",
            "id": c.get("id").and_then(Value::as_str).map(str::to_string)
                .unwrap_or_else(|| format!("call_{i}")),
            "name": c.pointer("/function/name").cloned().unwrap_or(Value::Null),
            "input": input,
        }));
    }

    let finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop");
    let (stop_reason, stop_details) = match finish {
        "tool_calls" | "function_call" => ("tool_use", None),
        "length" => ("max_tokens", None),
        "content_filter" => ("refusal", Some(json!({"category": "content_filter"}))),
        "error" => {
            return Err(Problem::Upstream(
                "the upstream provider reported finish_reason=error".into(),
            ));
        }
        // Some models say "stop" but still include tool calls.
        _ if !calls.is_empty() => ("tool_use", None),
        _ => ("end_turn", None),
    };

    let mut out = json!({
        "id": resp.get("id").cloned().unwrap_or(json!("")),
        "model": resp.get("model").and_then(Value::as_str).unwrap_or(model),
        "content": content,
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": resp.pointer("/usage/prompt_tokens").cloned().unwrap_or(json!(0)),
            "output_tokens": resp.pointer("/usage/completion_tokens").cloned().unwrap_or(json!(0)),
            "cache_read_input_tokens": resp.pointer("/usage/prompt_tokens_details/cached_tokens")
                .cloned().unwrap_or(Value::Null),
        }
    });
    if let Some(d) = stop_details {
        out["stop_details"] = d;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Effort;
    use crate::llm::types::{MAX_TOKENS, StopReason, ToolDefinition};

    fn request() -> MessagesRequest {
        MessagesRequest {
            model: "claude-opus-5-5".into(),
            max_tokens: MAX_TOKENS,
            system: "be safe".into(),
            tools: vec![ToolDefinition {
                name: "telemetry__search_logs".into(),
                description: "d".into(),
                input_schema: json!({"type": "object", "properties": {}}),
            }],
            messages: vec![
                json!({"role": "user", "content": "why is payments down?"}),
                json!({"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hmm", "signature": "s"},
                    {"type": "text", "text": "checking"},
                    {"type": "tool_use", "id": "toolu_1", "name": "telemetry__search_logs",
                     "input": {"service_name": "payments-api"}}]}),
                json!({"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1",
                     "content": "<tool_output>x</tool_output>", "is_error": true}]}),
            ],
            effort: Effort::Medium,
            fallbacks: true,
            progress_updates: true,
        }
    }

    #[test]
    fn real_request_is_translated_and_anthropic_only_fields_are_dropped() {
        let out = to_openrouter_body(
            &request().to_body(),
            "vendor/model:free",
            &["a/b".into()],
            false,
        );
        for dropped in ["thinking", "output_config", "fallbacks", "system"] {
            assert!(out.get(dropped).is_none(), "{dropped} leaked");
        }
        assert_eq!(out["model"], "vendor/model:free");
        assert_eq!(out["max_tokens"], 16000);
        assert_eq!(out["tool_choice"], "auto");
        assert_eq!(out["models"], json!(["vendor/model:free", "a/b"]));
        assert_eq!(out["provider"], json!({"require_parameters": true}));
        let m = out["messages"].as_array().unwrap();
        assert_eq!(m[0], json!({"role": "system", "content": "be safe"}));
        assert_eq!(m[2]["content"], "checking");
        assert_eq!(m[2]["tool_calls"][0]["id"], "toolu_1");
        assert_eq!(
            m[2]["tool_calls"][0]["function"]["arguments"],
            "{\"service_name\":\"payments-api\"}"
        );
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "toolu_1");
        assert!(
            m[3]["content"]
                .as_str()
                .unwrap()
                .starts_with("[tool error]")
        );
        assert_eq!(
            out["tools"][0]["function"]["name"],
            "telemetry__search_logs"
        );
        let deny = to_openrouter_body(&request().to_body(), "m", &[], true);
        assert_eq!(deny["provider"]["data_collection"], "deny");
        assert!(deny.get("models").is_none());
    }

    fn parse(v: Value) -> MessageResponse {
        let shaped = from_openrouter_response(&v, "m").ok().expect("translates");
        serde_json::from_value(shaped).expect("is a MessageResponse")
    }

    #[test]
    fn tool_calls_become_tool_use_blocks_the_runner_understands() {
        let r = parse(json!({"id": "g1", "model": "x/y",
            "usage": {"prompt_tokens": 5, "completion_tokens": 7,
                      "prompt_tokens_details": {"cached_tokens": 2}},
            "choices": [{"finish_reason": "tool_calls", "message": {"content": "looking", "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "t", "arguments": "{\"a\":1}"}},
                {"id": "c2", "type": "function", "function": {"name": "t", "arguments": "{oops"}}]}}]}));
        assert_eq!(r.stop(), StopReason::ToolUse);
        assert_eq!(r.texts(), ["looking"]);
        let uses = r.tool_uses().unwrap();
        assert_eq!(uses[0].input, json!({"a": 1}));
        assert_eq!(uses[1].input["_invalid_arguments"], "{oops");
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (5, 7));
        assert_eq!(r.usage.cache_read_input_tokens, Some(2));
    }

    #[test]
    fn finish_reasons_map_to_stop_reasons() {
        let mk = |f: &str| json!({"choices": [{"finish_reason": f, "message": {"content": "hi"}}]});
        assert_eq!(parse(mk("stop")).stop(), StopReason::EndTurn);
        assert_eq!(parse(mk("length")).stop(), StopReason::MaxTokens);
        assert_eq!(
            parse(mk("content_filter")).stop(),
            StopReason::Refusal(Some("content_filter".into()))
        );
        assert!(matches!(
            from_openrouter_response(&mk("error"), "m"),
            Err(Problem::Upstream(_))
        ));
        assert!(matches!(
            from_openrouter_response(&json!({"choices": []}), "m"),
            Err(Problem::Malformed(_))
        ));
    }

    #[test]
    fn a_reply_round_trips_through_history_into_the_next_request() {
        let r = parse(
            json!({"choices": [{"finish_reason": "tool_calls", "message": {
            "content": null, "tool_calls": [{"id": "c9", "type": "function",
            "function": {"name": "telemetry__search_logs", "arguments": "{\"service_name\":\"auth-service\"}"}}]}}]}),
        );
        let mut req = request();
        req.messages = vec![
            json!({"role": "user", "content": "go"}),
            json!({"role": "assistant", "content": r.content}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "c9", "content": "ok"}]}),
        ];
        let out = to_openrouter_body(&req.to_body(), "m", &[], false);
        let m = out["messages"].as_array().unwrap();
        assert_eq!(m[2]["tool_calls"][0]["id"], "c9");
        assert_eq!(m[2]["content"], Value::Null);
        assert_eq!(m[3]["tool_call_id"], "c9");
    }

    #[test]
    fn errors_are_classified() {
        let retry = |s| matches!(classify_error(s, "m".into(), None), Attempt::Retry(..));
        assert!(retry(429) && retry(500) && retry(502) && retry(503) && retry(408));
        assert!(!retry(400) && !retry(401) && !retry(402) && !retry(403));
        assert!(matches!(
            classify_error(429, "Rate limit exceeded: free-models-per-day".into(), None),
            Attempt::Done(Err(LlmError::RateLimited { .. }))
        ));
        assert!(matches!(
            classify_error(401, "bad key".into(), None),
            Attempt::Done(Err(LlmError::Auth(_)))
        ));
    }
}
