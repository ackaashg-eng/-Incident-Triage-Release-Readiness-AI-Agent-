//! The agentic loop.
//!
//! One user request ("turn") runs up to `max_iterations` model calls:
//!
//! ```text
//! user text → Claude → (tool_use blocks?) ──no──→ final answer
//!                         │ yes
//!                         ▼
//!            for EACH tool_use, in order:
//!              unknown tool?        → error result
//!              invalid arguments?   → error result (not executed)
//!              read-only            → execute on the owning MCP server
//!              requires approval    → Proposal → Approver (Phase 6) → run or not
//!            all tool_result blocks → ONE user message → call Claude again
//! ```
//!
//! History rules (Opus 5.5 "preserved thinking"):
//! * the conversation is append-only; assistant content is stored verbatim;
//! * the system prompt and tool list never change within a session;
//! * when a turn cannot complete (API error, refusal, truncated output,
//!   interruption) the history is cut back to where the turn started, which
//!   keeps the earlier prefix byte-identical.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::approval::{Approver, Decision, Proposal};
use super::prompt::{PromptContext, system_prompt};
use super::registry::{Permission, RegisteredTool, ToolRegistry, ToolTarget, resource_uri_allowed};
use super::validate::validate;
use crate::config::Effort;
use crate::llm::types::tool_result_block;
use crate::llm::{LlmClient, MessagesRequest, StopReason, ToolDefinition, ToolUse};
use crate::mcp::connection::McpError;
use crate::mcp::manager::ServerSet;

/// Tool output longer than this is cut (it all goes into Claude's context).
pub const MAX_RESULT_CHARS: usize = 30_000;

#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub model: String,
    pub effort: Effort,
    pub fallbacks: bool,
    pub progress_updates: bool,
    pub max_iterations: u32,
    pub max_tokens: u32,
    pub tool_timeout: Duration,
}

/// What happened, for the CLI (and later the trace log).
#[derive(Debug)]
pub enum AgentEvent<'a> {
    ModelCall {
        iteration: u32,
        max: u32,
    },
    ModelReply {
        stop: &'a StopReason,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
    },
    Note(&'a str),
    Text(&'a str),
    Fallback(&'a str),
    ToolStart {
        name: &'a str,
        server: &'a str,
        permission: Permission,
        input: &'a Value,
    },
    ToolEnd {
        name: &'a str,
        ok: bool,
        summary: &'a str,
        millis: u128,
    },
    Proposal {
        proposal: &'a Proposal,
        decision: &'a Decision,
    },
    Warning(&'a str),
}

pub trait Observer: Send + Sync {
    fn on_event(&self, event: &AgentEvent<'_>);
}

pub struct NoopObserver;

impl Observer for NoopObserver {
    fn on_event(&self, _event: &AgentEvent<'_>) {}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Answered(String),
    /// Declined by a safety classifier (category if known).
    Refused(Option<String>),
    /// The reply hit `max_tokens`; tools in it were not run.
    Truncated,
    IterationLimit(u32),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnReport {
    pub outcome: TurnOutcome,
    pub model_calls: u32,
    pub tool_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub struct Agent<L: LlmClient> {
    llm: L,
    settings: AgentSettings,
    system: String,
    tools: Vec<ToolDefinition>,
    registry: ToolRegistry,
    servers: ServerSet,
    approver: Box<dyn Approver>,
    history: Vec<Value>,
    turn_start: Option<usize>,
}

impl<L: LlmClient> Agent<L> {
    pub fn new(
        llm: L,
        settings: AgentSettings,
        servers: ServerSet,
        reference_time: &str,
        approver: Box<dyn Approver>,
    ) -> Self {
        let registry = ToolRegistry::build(servers.discovered());
        let connected: Vec<(String, String)> = servers
            .discovered()
            .iter()
            .map(|s| (s.name.to_string(), s.label.clone()))
            .collect();
        let system = system_prompt(&PromptContext {
            connected: &connected,
            unavailable: servers.unavailable(),
            reference_time,
        });
        Self {
            llm,
            settings,
            system,
            tools: registry.claude_tools(),
            registry,
            servers,
            approver,
            history: Vec::new(),
            turn_start: None,
        }
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub fn servers(&self) -> &ServerSet {
        &self.servers
    }

    pub fn history(&self) -> &[Value] {
        &self.history
    }

    pub fn system_prompt(&self) -> &str {
        &self.system
    }

    /// `/reset`: forget the conversation (tools and servers stay).
    pub fn reset(&mut self) {
        self.history.clear();
        self.turn_start = None;
    }

    /// After a turn was cancelled mid-way (Ctrl+C), drop its partial messages.
    /// Returns true if anything was rolled back.
    pub fn recover_interrupted(&mut self) -> bool {
        match self.turn_start.take() {
            Some(start) => {
                self.history.truncate(start);
                true
            }
            None => false,
        }
    }

    pub async fn shutdown(self) {
        self.servers.shutdown().await;
    }

    /// Runs one user request to completion (or to a limit).
    pub async fn run_turn(&mut self, user_text: &str, obs: &dyn Observer) -> TurnReport {
        self.recover_interrupted();
        let start = self.history.len();
        self.turn_start = Some(start);
        self.history
            .push(json!({"role": "user", "content": [{"type": "text", "text": user_text}]}));

        let mut report = TurnReport {
            outcome: TurnOutcome::Failed("not started".into()),
            model_calls: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        let max = self.settings.max_iterations;

        for iteration in 1..=max {
            obs.on_event(&AgentEvent::ModelCall { iteration, max });
            let request = MessagesRequest {
                model: self.settings.model.clone(),
                max_tokens: self.settings.max_tokens,
                system: self.system.clone(),
                tools: self.tools.clone(),
                messages: self.history.clone(),
                effort: self.settings.effort,
                fallbacks: self.settings.fallbacks,
                progress_updates: self.settings.progress_updates,
            };
            report.model_calls += 1;
            let response = match self.llm.send(&request).await {
                Ok(r) => r,
                Err(err) => return self.abort(report, TurnOutcome::Failed(err.to_string())),
            };

            let stop = response.stop();
            report.input_tokens += response.usage.input_tokens;
            report.output_tokens += response.usage.output_tokens;
            obs.on_event(&AgentEvent::ModelReply {
                stop: &stop,
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cache_read: response.usage.cache_read_input_tokens.unwrap_or(0),
            });
            for switch in response.fallback_switches() {
                obs.on_event(&AgentEvent::Fallback(&switch));
            }
            for block in &response.content {
                match block.get("type").and_then(Value::as_str) {
                    Some("thinking") => {
                        if let Some(note) =
                            block.get("thinking").and_then(Value::as_str).map(str::trim)
                            && !note.is_empty()
                        {
                            obs.on_event(&AgentEvent::Note(note));
                        }
                    }
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            obs.on_event(&AgentEvent::Text(text));
                        }
                    }
                    _ => {}
                }
            }

            // Never run tools from a refused or truncated reply: their input may be cut off.
            match &stop {
                StopReason::Refusal(category) => {
                    return self.abort(report, TurnOutcome::Refused(category.clone()));
                }
                StopReason::MaxTokens => return self.abort(report, TurnOutcome::Truncated),
                _ => {}
            }
            let tool_uses = match response.tool_uses() {
                Ok(t) => t,
                Err(e) => {
                    return self.abort(
                        report,
                        TurnOutcome::Failed(format!("malformed model response: {e}")),
                    );
                }
            };

            if response.content.is_empty() {
                // Keep user/assistant alternation without inventing content.
                self.history.push(json!({"role": "assistant", "content": [{"type": "text", "text": "(no answer)"}]}));
                return self.finish(report, TurnOutcome::Answered(String::new()));
            }
            self.history
                .push(json!({"role": "assistant", "content": response.content}));

            if tool_uses.is_empty() {
                if stop == StopReason::PauseTurn {
                    continue;
                }
                let answer = response.texts().join("\n");
                return self.finish(report, TurnOutcome::Answered(answer));
            }

            let mut results = Vec::with_capacity(tool_uses.len());
            for tool_use in &tool_uses {
                report.tool_calls += 1;
                results.push(self.execute(tool_use, obs).await);
            }
            self.history
                .push(json!({"role": "user", "content": results}));
        }

        // Out of iterations: close the turn with a visible note so the
        // history ends with an assistant message and the next turn is valid.
        self.history.push(json!({"role": "assistant", "content": [{"type": "text", "text": format!(
            "[Stopped: the limit of {max} model calls for one request was reached. The last tool results were received but not analysed.]"
        )}]}));
        self.finish(report, TurnOutcome::IterationLimit(max))
    }

    fn finish(&mut self, mut report: TurnReport, outcome: TurnOutcome) -> TurnReport {
        self.turn_start = None;
        report.outcome = outcome;
        report
    }

    fn abort(&mut self, report: TurnReport, outcome: TurnOutcome) -> TurnReport {
        self.recover_interrupted();
        self.finish(report, outcome)
    }

    /// Runs (or refuses) one tool call and returns its `tool_result` block.
    async fn execute(&self, call: &ToolUse, obs: &dyn Observer) -> Value {
        let started = Instant::now();
        let Some(tool) = self.registry.get(&call.name) else {
            let known: Vec<&str> = self.registry.iter().map(|t| t.name.as_str()).collect();
            let message = format!(
                "Unknown tool `{}`. Available tools: {}.",
                call.name,
                known.join(", ")
            );
            obs.on_event(&AgentEvent::Warning(&message));
            return tool_result_block(&call.id, &message, true);
        };
        obs.on_event(&AgentEvent::ToolStart {
            name: &tool.name,
            server: tool.server,
            permission: tool.permission,
            input: &call.input,
        });

        let (content, is_error) = self.run_tool(tool, &call.input, obs).await;
        let summary = summarise(&content, is_error);
        obs.on_event(&AgentEvent::ToolEnd {
            name: &tool.name,
            ok: !is_error,
            summary: &summary,
            millis: started.elapsed().as_millis(),
        });
        tool_result_block(&call.id, &content, is_error)
    }

    async fn run_tool(
        &self,
        tool: &RegisteredTool,
        input: &Value,
        obs: &dyn Observer,
    ) -> (String, bool) {
        let problems = validate(input, &tool.mcp_schema);
        if !problems.is_empty() {
            return (
                format!(
                    "Invalid arguments for {} - the tool was NOT run: {}",
                    tool.name,
                    problems.join("; ")
                ),
                true,
            );
        }
        match tool.permission {
            Permission::Blocked => (
                format!("{} is blocked by policy and was not run.", tool.name),
                true,
            ),
            Permission::ReadOnly => self.call_server(tool, input).await,
            Permission::RequiresApproval => {
                let proposal = Proposal {
                    tool: tool.name.clone(),
                    server: tool.server,
                    arguments: input.clone(),
                };
                let decision = self.approver.review(&proposal).await;
                obs.on_event(&AgentEvent::Proposal {
                    proposal: &proposal,
                    decision: &decision,
                });
                match decision {
                    Decision::Approved(arguments) => {
                        let problems = validate(&arguments, &tool.mcp_schema);
                        if problems.is_empty() {
                            self.call_server(tool, &arguments).await
                        } else {
                            (
                                format!(
                                    "The approved arguments are invalid - NOT executed: {}",
                                    problems.join("; ")
                                ),
                                true,
                            )
                        }
                    }
                    Decision::Rejected(reason) => (
                        format!(
                            "NOT EXECUTED: the human reviewer rejected this action. Reason: {reason}"
                        ),
                        true,
                    ),
                    Decision::Unavailable(reason) => (
                        format!(
                            "NOT EXECUTED: {reason}. The proposal was shown to the user. Do not claim the action \
                             happened; present it as a proposal awaiting approval."
                        ),
                        true,
                    ),
                }
            }
        }
    }

    async fn call_server(&self, tool: &RegisteredTool, input: &Value) -> (String, bool) {
        let Some(conn) = self.servers.get(tool.server) else {
            return (
                format!(
                    "The {} server is not connected in this session.",
                    tool.server
                ),
                true,
            );
        };
        let timeout = self.settings.tool_timeout;
        match &tool.target {
            ToolTarget::McpTool(mcp_name) => {
                let arguments = input.as_object().cloned().unwrap_or_default();
                match conn.call_tool(mcp_name, arguments, timeout).await {
                    Ok(result) => (
                        truncate(render_call_result(&result)),
                        result.is_error == Some(true),
                    ),
                    Err(err) => (describe_mcp_error(&err), true),
                }
            }
            ToolTarget::ReadResource { uris, templates } => {
                let uri = input.get("uri").and_then(Value::as_str).unwrap_or_default();
                if !resource_uri_allowed(uri, uris, templates) {
                    let mut valid = uris.clone();
                    valid.extend(templates.iter().cloned());
                    return (
                        format!("Unknown resource URI `{uri}`. Valid: {}.", valid.join(", ")),
                        true,
                    );
                }
                match conn.read_resource(uri, timeout).await {
                    Ok(result) => {
                        let text = result
                            .contents
                            .iter()
                            .map(|c| match c {
                                rmcp::model::ResourceContents::TextResourceContents {
                                    text,
                                    ..
                                } => text.clone(),
                                _ => "[binary resource content omitted]".to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        (truncate(text), false)
                    }
                    Err(err) => (describe_mcp_error(&err), true),
                }
            }
        }
    }
}

/// The text parts of a tool result. MCP servers put the human/model-readable
/// form there (our telemetry server: the JSON body; the filesystem server: the
/// raw file). Structured content is only used when there is no text, because
/// the filesystem server's `{"content": "..."}` would turn a file into one
/// escaped string.
pub fn render_call_result(result: &rmcp::model::CallToolResult) -> String {
    let texts: Vec<&str> = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect();
    let non_text = result.content.len() - texts.len();
    let mut out = texts.join("\n");
    if out.is_empty()
        && let Some(structured) = &result.structured_content
    {
        out = structured.to_string();
    }
    if non_text > 0 {
        out.push_str(&format!("\n[{non_text} non-text content block(s) omitted]"));
    }
    if out.trim().is_empty() {
        "(empty result)".to_string()
    } else {
        out
    }
}
fn describe_mcp_error(err: &McpError) -> String {
    match err {
        McpError::Rejected { message, data, .. } => {
            match data.as_ref().and_then(|d| d.get("error")) {
                Some(detail) => format!("{message} {detail}"),
                None => message.clone(),
            }
        }
        McpError::Disconnected { server, .. } => {
            format!(
                "The {server} server has disconnected; its tools are unavailable for the rest of this session."
            )
        }
        other => other.to_string(),
    }
}

fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!("{cut}\n[... truncated: the result was longer than {MAX_RESULT_CHARS} characters]")
}

/// One short line for the CLI.
pub fn summarise(content: &str, is_error: bool) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(content) {
        if let Some(err) = v.get("error") {
            let code = err.get("code").and_then(Value::as_str).unwrap_or("error");
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("");
            return format!("{code}: {msg}");
        }
        if let (Some(r), Some(t)) = (v.get("returned"), v.get("total_matching")) {
            return format!("{r} of {t} matching log entries");
        }
        if let Some(c) = v.get("count") {
            return format!("{c} deployment(s) in the window");
        }
        if let Some(id) = v.pointer("/ticket/ticket_id") {
            return format!("ticket {} created", id.as_str().unwrap_or("?"));
        }
    }
    let first = content.lines().next().unwrap_or("").trim();
    let first: String = first.chars().take(100).collect();
    if is_error {
        first
    } else {
        format!("{} chars: {first}", content.chars().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        assert_eq!(
            summarise(r#"{"returned":3,"total_matching":3}"#, false),
            "3 of 3 matching log entries"
        );
        assert_eq!(
            summarise(r#"{"count":1}"#, false),
            "1 deployment(s) in the window"
        );
        assert_eq!(
            summarise(
                r#"{"error":{"code":"UNKNOWN_SERVICE","message":"no"}}"#,
                true
            ),
            "UNKNOWN_SERVICE: no"
        );
        assert!(summarise("# Runbook\nline", false).starts_with("14 chars: # Runbook"));
    }

    #[test]
    fn long_results_are_truncated() {
        let long = "x".repeat(MAX_RESULT_CHARS + 10);
        let out = truncate(long);
        assert!(out.ends_with("characters]"));
        assert!(out.chars().count() < MAX_RESULT_CHARS + 100);
    }
}
