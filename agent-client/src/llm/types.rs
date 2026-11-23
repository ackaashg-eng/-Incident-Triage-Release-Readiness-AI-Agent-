//! Messages API request construction and response parsing.
//!
//! Conversation messages are kept as raw `serde_json::Value`s on purpose: the
//! assistant's `content` must be sent back **byte-for-byte unchanged** in the
//! next request (Opus 5.5 "preserved thinking": thinking blocks are bound to
//! the conversation). Parsing into typed structs and re-serialising could drop
//! or reorder fields, so we only *read* typed views and *store* the raw JSON.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Effort;

pub const API_VERSION: &str = "2026-10-21";
pub const BETA_FALLBACKS: &str = "server-side-fallback-2026-07-01";
pub const BETA_PROGRESS_UPDATES: &str = "thinking-display-updates-2026-08-18";
/// Room for adaptive thinking plus the reply, small enough for a non-streaming call.
pub const MAX_TOKENS: u32 = 16_000;

/// A tool as Claude sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Everything needed for one `POST /v1/messages`.
#[derive(Debug, Clone)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub tools: Vec<ToolDefinition>,
    pub messages: Vec<Value>,
    pub effort: Effort,
    pub fallbacks: bool,
    pub progress_updates: bool,
}

impl MessagesRequest {
    /// The JSON body. Rules for Claude Opus 5.5:
    /// * no `temperature`/`top_p`/`top_k` (rejected);
    /// * `thinking` omitted (always adaptive) unless we ask for progress updates;
    /// * depth via `output_config.effort`;
    /// * `tool_choice` only `auto` (forced tool use is a 400 on this model);
    /// * the system prompt is cached (tools + system form a stable prefix).
    pub fn to_body(&self) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": [{
                "type": "text",
                "text": self.system,
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": self.messages,
            "output_config": {"effort": self.effort.to_string()},
        });
        if !self.tools.is_empty() {
            body["tools"] = json!(self.tools);
            body["tool_choice"] = json!({"type": "auto"});
        }
        if self.progress_updates {
            body["thinking"] = json!({"type": "adaptive", "display": "updates"});
        }
        if self.fallbacks {
            body["fallbacks"] = json!("default");
        }
        body
    }

    /// Value for the `anthropic-beta` header (empty = no header).
    pub fn beta_header(&self) -> String {
        let mut betas = Vec::new();
        if self.fallbacks {
            betas.push(BETA_FALLBACKS);
        }
        if self.progress_updates {
            betas.push(BETA_PROGRESS_UPDATES);
        }
        betas.join(",")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
}

/// A successful Messages API response.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    /// Raw content blocks, appended to the history exactly as received.
    pub content: Vec<Value>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_details: Option<Value>,
    #[serde(default)]
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// Declined by a safety classifier (`stop_details.category`, if given).
    Refusal(Option<String>),
    PauseTurn,
    StopSequence,
    Other(String),
    Missing,
}

/// One `tool_use` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

impl MessageResponse {
    pub fn stop(&self) -> StopReason {
        match self.stop_reason.as_deref() {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("refusal") => StopReason::Refusal(
                self.stop_details
                    .as_ref()
                    .and_then(|d| d.get("category"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ),
            Some("pause_turn") => StopReason::PauseTurn,
            Some("stop_sequence") => StopReason::StopSequence,
            Some(other) => StopReason::Other(other.to_string()),
            None => StopReason::Missing,
        }
    }

    /// Text blocks, in order.
    pub fn texts(&self) -> Vec<&str> {
        self.blocks_of("text")
            .filter_map(|b| b.get("text")?.as_str())
            .collect()
    }

    /// Non-empty progress notes (thinking blocks shown with `display: "updates"`).
    pub fn progress_notes(&self) -> Vec<&str> {
        self.blocks_of("thinking")
            .filter_map(|b| b.get("thinking")?.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// `"<from> declined; <to> continued"` for each server-side fallback switch.
    pub fn fallback_switches(&self) -> Vec<String> {
        self.blocks_of("fallback")
            .map(|b| {
                let model = |k: &str| {
                    b.get(k)
                        .and_then(|m| m.get("model"))
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                };
                format!("{} declined; {} continued", model("from"), model("to"))
            })
            .collect()
    }

    /// All `tool_use` blocks. A block without an id or name is malformed.
    pub fn tool_uses(&self) -> Result<Vec<ToolUse>, String> {
        self.blocks_of("tool_use")
            .map(|b| {
                let id = b
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                let name = b
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                match (id, name) {
                    (Some(id), Some(name)) => Ok(ToolUse {
                        id: id.to_string(),
                        name: name.to_string(),
                        input: b.get("input").cloned().unwrap_or(Value::Null),
                    }),
                    _ => Err(format!("tool_use block without id or name: {b}")),
                }
            })
            .collect()
    }

    fn blocks_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.content
            .iter()
            .filter(move |b| b.get("type").and_then(Value::as_str) == Some(kind))
    }
}

/// `{"type": "tool_result", ...}` block for one tool call.
pub fn tool_result_block(tool_use_id: &str, content: &str, is_error: bool) -> Value {
    let mut block = json!({
        "type": "tool_result",
        "tool_use_id": tool_use_id,
        "content": content,
    });
    if is_error {
        block["is_error"] = json!(true);
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(fallbacks: bool, updates: bool) -> MessagesRequest {
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
            fallbacks,
            progress_updates: updates,
        }
    }

    #[test]
    fn body_follows_the_opus_5_5_rules() {
        let body = request(true, true).to_body();
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["max_tokens"], 16000);
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["tool_choice"], json!({"type": "auto"}));
        assert_eq!(body["output_config"]["effort"], "medium");
        assert_eq!(
            body["thinking"],
            json!({"type": "adaptive", "display": "updates"})
        );
        assert_eq!(body["fallbacks"], "default");
        for forbidden in ["temperature", "top_p", "top_k"] {
            assert!(body.get(forbidden).is_none(), "{forbidden}");
        }
        assert_eq!(
            request(true, true).beta_header(),
            "server-side-fallback-2026-07-01,thinking-display-updates-2026-08-18"
        );
    }

    #[test]
    fn optional_parts_are_omitted() {
        let mut r = request(false, false);
        r.tools.clear();
        let body = r.to_body();
        for key in ["thinking", "fallbacks", "tools", "tool_choice"] {
            assert!(body.get(key).is_none(), "{key}");
        }
        assert_eq!(r.beta_header(), "");
    }

    #[test]
    fn response_views() {
        let resp: MessageResponse = serde_json::from_value(json!({
            "id": "msg_1", "model": "claude-opus-5-5", "stop_reason": "tool_use",
            "content": [
                {"type": "thinking", "thinking": "", "signature": "abc"},
                {"type": "thinking", "thinking": "Checking the logs first.", "signature": "def"},
                {"type": "text", "text": "Looking."},
                {"type": "tool_use", "id": "toolu_1", "name": "telemetry__search_logs", "input": {"service_name": "x"}},
                {"type": "tool_use", "id": "toolu_2", "name": "files__read_text_file", "input": {"path": "a"}}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 3}
        }))
        .unwrap();
        assert_eq!(resp.stop(), StopReason::ToolUse);
        assert_eq!(resp.texts(), ["Looking."]);
        assert_eq!(resp.progress_notes(), ["Checking the logs first."]);
        let uses = resp.tool_uses().unwrap();
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[1].name, "files__read_text_file");
        assert_eq!(resp.usage.cache_read_input_tokens, Some(3));
    }

    #[test]
    fn refusal_category_and_fallbacks_are_read() {
        let resp: MessageResponse = serde_json::from_value(json!({
            "content": [{"type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-4-8"}}],
            "stop_reason": "refusal", "stop_details": {"category": "cyber"}
        }))
        .unwrap();
        assert_eq!(resp.stop(), StopReason::Refusal(Some("cyber".into())));
        assert_eq!(
            resp.fallback_switches(),
            ["claude-opus-5-5 declined; claude-opus-4-8 continued"]
        );
    }

    #[test]
    fn malformed_tool_use_is_detected() {
        let resp: MessageResponse = serde_json::from_value(
            json!({"content": [{"type": "tool_use", "input": {}}], "stop_reason": "tool_use"}),
        )
        .unwrap();
        assert!(resp.tool_uses().is_err());
    }

    #[test]
    fn tool_result_blocks() {
        assert_eq!(
            tool_result_block("t1", "ok", false),
            json!({"type": "tool_result", "tool_use_id": "t1", "content": "ok"})
        );
        assert_eq!(tool_result_block("t1", "bad", true)["is_error"], true);
    }
}
