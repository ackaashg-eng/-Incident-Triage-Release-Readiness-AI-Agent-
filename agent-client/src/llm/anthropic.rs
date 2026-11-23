//! HTTP client for `POST /v1/messages`.
//!
//! * Headers: `x-api-key`, `anthropic-version`, optional `anthropic-beta`.
//! * Timeouts: one per HTTP request (configurable) plus a connect timeout.
//! * Retries (up to 3): 429 rate limit, 500/502/503/504, 529 overloaded,
//!   network errors and timeouts. `retry-after` is honoured (capped);
//!   otherwise exponential backoff with jitter. 400/401/403/404/413 are not
//!   retried: they will not fix themselves.
//! * The API key only ever goes into the `x-api-key` header. It is not in any
//!   error message or log line.

use std::future::Future;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;
use thiserror::Error;

use super::types::{API_VERSION, MessageResponse, MessagesRequest};
use crate::config::ApiKey;

const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LlmError {
    #[error(
        "the API key was rejected (401): {0}. Run scripts\\setup.ps1 -ResetKey to paste a valid key."
    )]
    Auth(String),
    #[error("the API key is not allowed to do this (403): {0}")]
    Permission(String),
    #[error("Claude rejected the request ({status}): {message}")]
    InvalidRequest { status: u16, message: String },
    #[error("rate limited by the Claude API (429) after {attempts} attempt(s): {message}")]
    RateLimited { attempts: u32, message: String },
    #[error(
        "the Claude API is overloaded (529) after {attempts} attempt(s); try again in a minute"
    )]
    Overloaded { attempts: u32 },
    #[error("Claude API server error ({status}) after {attempts} attempt(s): {message}")]
    Server {
        status: u16,
        attempts: u32,
        message: String,
    },
    #[error("network error talking to the Claude API after {attempts} attempt(s): {message}")]
    Network { attempts: u32, message: String },
    #[error("the Claude API did not answer within {secs}s (after {attempts} attempt(s))")]
    Timeout { secs: u64, attempts: u32 },
    #[error("the Claude API returned a response that could not be understood: {0}")]
    Malformed(String),
}

/// Anything that can answer a Messages request: the real API, or a scripted
/// fake in tests.
pub trait LlmClient: Send + Sync {
    fn send(
        &self,
        request: &MessagesRequest,
    ) -> impl Future<Output = Result<MessageResponse, LlmError>> + Send;
}

pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: ApiKey,
    url: String,
    timeout: Duration,
    max_retries: u32,
    base_delay: Duration,
}

impl AnthropicClient {
    pub fn new(api_key: ApiKey, base_url: &str, timeout: Duration) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("incident-triage-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| LlmError::Network {
                attempts: 0,
                message: e.to_string(),
            })?;
        Ok(Self {
            http,
            api_key,
            url: format!("{}/v1/messages", base_url.trim_end_matches('/')),
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

    async fn send_once(&self, request: &MessagesRequest) -> Attempt {
        let mut builder = self
            .http
            .post(&self.url)
            .header("x-api-key", self.api_key.expose())
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&request.to_body());
        let beta = request.beta_header();
        if !beta.is_empty() {
            builder = builder.header("anthropic-beta", beta);
        }

        let response = match builder.send().await {
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
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) if e.is_timeout() => return Attempt::Retry(Failure::Timeout, None),
            Err(e) => return Attempt::Retry(Failure::Network(describe(e)), None),
        };

        if status.is_success() {
            return match serde_json::from_slice::<MessageResponse>(&bytes) {
                Ok(msg) => {
                    tracing::debug!(%request_id, id = %msg.id, stop = ?msg.stop_reason, "Claude response");
                    Attempt::Done(Ok(msg))
                }
                Err(e) => Attempt::Done(Err(LlmError::Malformed(format!(
                    "{e} (request-id {request_id})"
                )))),
            };
        }

        let message = error_message(&bytes, &request_id);
        tracing::warn!(status = status.as_u16(), %request_id, %message, "Claude API error");
        match status {
            StatusCode::UNAUTHORIZED => Attempt::Done(Err(LlmError::Auth(message))),
            StatusCode::FORBIDDEN => Attempt::Done(Err(LlmError::Permission(message))),
            StatusCode::TOO_MANY_REQUESTS => {
                Attempt::Retry(Failure::RateLimited(message), retry_after)
            }
            s if s.as_u16() == 529 => Attempt::Retry(Failure::Overloaded, retry_after),
            s if s.is_server_error() => {
                Attempt::Retry(Failure::Server(s.as_u16(), message), retry_after)
            }
            s => Attempt::Done(Err(LlmError::InvalidRequest {
                status: s.as_u16(),
                message,
            })),
        }
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let exp = self
            .base_delay
            .saturating_mul(2u32.saturating_pow(attempt))
            .min(MAX_BACKOFF);
        // Up to 25% jitter so parallel clients don't retry in lock-step.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        exp + exp.mul_f64(f64::from(nanos % 1000) / 4000.0)
    }
}

impl LlmClient for AnthropicClient {
    async fn send(&self, request: &MessagesRequest) -> Result<MessageResponse, LlmError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (failure, retry_after) = match self.send_once(request).await {
                Attempt::Done(result) => return result,
                Attempt::Retry(failure, retry_after) => (failure, retry_after),
            };
            if attempt > self.max_retries {
                return Err(failure.into_error(attempt, self.timeout));
            }
            let delay = retry_after.unwrap_or_else(|| self.backoff(attempt - 1));
            tracing::warn!(attempt, ?delay, failure = ?failure, "retrying the Claude API request");
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
    Overloaded,
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
            Self::Overloaded => LlmError::Overloaded { attempts },
            Self::Server(status, message) => LlmError::Server {
                status,
                attempts,
                message,
            },
        }
    }
}

/// `{"type":"error","error":{"type":"...","message":"..."}}` → "type: message".
fn error_message(body: &[u8], request_id: &str) -> String {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let detail = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .map(|e| {
            let kind = e.get("type").and_then(Value::as_str).unwrap_or("error");
            let msg = e.get("message").and_then(Value::as_str).unwrap_or("");
            format!("{kind}: {msg}")
        })
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body);
            text.chars().take(300).collect()
        });
    if request_id.is_empty() {
        detail
    } else {
        format!("{detail} (request-id {request_id})")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_bodies_are_summarised() {
        let body = br#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: too large"}}"#;
        assert_eq!(
            error_message(body, "req_1"),
            "invalid_request_error: max_tokens: too large (request-id req_1)"
        );
        assert_eq!(
            error_message(b"<html>bad gateway</html>", ""),
            "<html>bad gateway</html>"
        );
    }

    #[test]
    fn error_messages_never_contain_the_key() {
        let err = LlmError::Auth("authentication_error: invalid x-api-key".into());
        assert!(err.to_string().contains("setup.ps1"));
    }
}
