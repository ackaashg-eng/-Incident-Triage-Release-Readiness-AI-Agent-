//! The agentic loop, with its guardrails.
//!
//! One user request ("turn") runs up to `max_iterations` model calls:
//!
//! ```text
//! user text → Claude → (tool_use blocks?) ──no──→ final answer
//!                         │ yes
//!                         ▼
//!            for EACH tool_use, in order:
//!              unknown tool          → error result
//!              invalid arguments     → error result (never executed)
//!              read-only (policy)    → execute on the owning server (per-tool timeout)
//!              state-changing        → PAUSE → Proposal → human approves / rejects / edits
//!                                      → execute ONLY the approved payload (hash-checked)
//!              server output         → secrets redacted, injection scan, labelled untrusted
//!            all tool_result blocks → ONE user message → call Claude again
//! ```
//!
//! History rules (Opus 5.5 "preserved thinking"):
//! * the conversation is append-only; assistant content is stored verbatim;
//! * the system prompt and tool list never change within a session;
//! * when a turn cannot complete (API error, refusal, truncated output,
//!   interruption) the history is cut back to where the turn started.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::approval::{Approver, Decision, Proposal, payload_hash};
use super::prompt::{PromptContext, system_prompt};
use super::registry::{Permission, RegisteredTool, ToolRegistry, ToolTarget, resource_uri_allowed};
use super::validate::validate;
use crate::config::Effort;
use crate::llm::types::tool_result_block;
use crate::llm::{LlmClient, MessagesRequest, StopReason, ToolDefinition, ToolUse};
use crate::mcp::connection::McpError;
use crate::mcp::manager::ServerSet;
use crate::safety::injection::{scan, wrap_untrusted};
use crate::safety::redact::Redactor;

/// Tool output longer than this is cut (it all goes into Claude's context).
pub const MAX_RESULT_CHARS: usize = 30_000;

#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub model: String,
    pub effort: Effort,
    pub fallbacks: bool,
    pub progress_updates: bool,
    /// Maximum model calls per user request.
    pub max_iterations: u32,
    pub max_tokens: u32,
    /// Default per-tool execution timeout.
    pub tool_timeout: Duration,
    /// Per-tool overrides, by namespaced tool name.
    pub tool_timeouts: BTreeMap<String, Duration>,
    /// Exact secret values to remove from anything shown or logged.
    pub secrets: Vec<String>,
    /// (local path, placeholder) pairs, longest first, e.g. the project folder → `<project>`.
    pub path_aliases: Vec<(String, String)>,
}

impl AgentSettings {
    /// The redactor for everything this agent shows, sends or logs.
    pub fn redactor(&self) -> Redactor {
        self.path_aliases.iter().fold(
            Redactor::new(self.secrets.clone()),
            |r, (path, placeholder)| r.with_alias(path, placeholder),
        )
    }

    pub fn timeout_for(&self, tool: &str) -> Duration {
        self.tool_timeouts
            .get(tool)
            .copied()
            .unwrap_or(self.tool_timeout)
    }
}

/// What happened to one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Ok,
    /// The server ran it and reported an error (e.g. access denied, unknown service).
    ToolError,
    InvalidArguments,
    UnknownTool,
    Blocked,
    /// A human rejected it.
    Rejected,
    /// Not run for another reason (no reviewer, payload mismatch).
    NotExecuted,
    Timeout,
    Disconnected,
    Failed,
}

impl ToolStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::ToolError => "tool_error",
            Self::InvalidArguments => "invalid_arguments",
            Self::UnknownTool => "unknown_tool",
            Self::Blocked => "blocked",
            Self::Rejected => "rejected",
            Self::NotExecuted => "not_executed",
            Self::Timeout => "timeout",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    /// Read-only tool.
    NotRequired,
    Approved,
    ApprovedAfterEdit,
    Rejected,
    /// No reviewer available.
    Unavailable,
    /// The payload to execute differs from the approved one.
    PayloadMismatch,
    /// Invalid arguments: never shown to a reviewer.
    NotReached,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Approved => "approved",
            Self::ApprovedAfterEdit => "approved_after_edit",
            Self::Rejected => "rejected",
            Self::Unavailable => "unavailable",
            Self::PayloadMismatch => "payload_mismatch_blocked",
            Self::NotReached => "not_reached",
        }
    }
}

/// Everything about one tool call (for the console and the trace).
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool_use_id: String,
    pub name: String,
    pub server: String,
    pub permission: Option<Permission>,
    pub input: Value,
    pub input_sha256: String,
    /// The payload actually executed, when a reviewer edited it.
    pub executed_input: Option<Value>,
    pub status: ToolStatus,
    pub error: Option<String>,
    pub approval: ApprovalStatus,
    pub approved_payload_sha256: Option<String>,
    pub reviewer: Option<String>,
    pub injection_flags: Vec<&'static str>,
    pub redactions: usize,
    pub duration_ms: u128,
    pub output_chars: usize,
    pub summary: String,
}

/// What happened, for the console and the trace.
#[derive(Debug)]
pub enum AgentEvent<'a> {
    TurnStart {
        turn: u32,
        text: &'a str,
    },
    ModelCall {
        iteration: u32,
        max: u32,
    },
    ModelReply {
        stop: &'a StopReason,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        millis: u128,
    },
    ModelError {
        error: &'a str,
        millis: u128,
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
    Approval {
        proposal: &'a Proposal,
        decision: &'a Decision,
    },
    ToolEnd(&'a ToolCallRecord),
    Warning(&'a str),
    TurnEnd(&'a TurnReport),
}

pub trait Observer: Send + Sync {
    fn on_event(&self, event: &AgentEvent<'_>);
}

pub struct NoopObserver;

impl Observer for NoopObserver {
    fn on_event(&self, _event: &AgentEvent<'_>) {}
}

/// Sends every event to several observers (e.g. console + trace file).
pub struct Fanout<'a>(pub Vec<&'a dyn Observer>);

impl Observer for Fanout<'_> {
    fn on_event(&self, event: &AgentEvent<'_>) {
        for o in &self.0 {
            o.on_event(event);
        }
    }
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
    redactor: Redactor,
    history: Vec<Value>,
    turn_start: Option<usize>,
    turn: u32,
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
        let redactor = settings.redactor();
        Self {
            llm,
            system,
            tools: registry.claude_tools(),
            registry,
            servers,
            approver,
            redactor,
            settings,
            history: Vec::new(),
            turn_start: None,
            turn: 0,
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

    pub fn settings(&self) -> &AgentSettings {
        &self.settings
    }

    /// `/reset`: forget the conversation (tools and servers stay).
    pub fn reset(&mut self) {
        self.history.clear();
        self.turn_start = None;
    }

    /// After a turn was cancelled mid-way (Ctrl+C), drop its partial messages.
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
        self.turn += 1;
        obs.on_event(&AgentEvent::TurnStart {
            turn: self.turn,
            text: user_text,
        });
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
            let started = Instant::now();
            let response = match self.llm.send(&request).await {
                Ok(r) => r,
                Err(err) => {
                    let message = err.to_string();
                    obs.on_event(&AgentEvent::ModelError {
                        error: &message,
                        millis: started.elapsed().as_millis(),
                    });
                    return self.abort(report, TurnOutcome::Failed(message), obs);
                }
            };

            let stop = response.stop();
            report.input_tokens += response.usage.input_tokens;
            report.output_tokens += response.usage.output_tokens;
            obs.on_event(&AgentEvent::ModelReply {
                stop: &stop,
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cache_read: response.usage.cache_read_input_tokens.unwrap_or(0),
                millis: started.elapsed().as_millis(),
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
                    return self.abort(report, TurnOutcome::Refused(category.clone()), obs);
                }
                StopReason::MaxTokens => return self.abort(report, TurnOutcome::Truncated, obs),
                _ => {}
            }
            let tool_uses = match response.tool_uses() {
                Ok(t) => t,
                Err(e) => {
                    return self.abort(
                        report,
                        TurnOutcome::Failed(format!("malformed model response: {e}")),
                        obs,
                    );
                }
            };

            if response.content.is_empty() {
                // Keep user/assistant alternation without inventing content.
                self.history.push(json!({"role": "assistant", "content": [{"type": "text", "text": "(no answer)"}]}));
                return self.finish(report, TurnOutcome::Answered(String::new()), obs);
            }
            self.history
                .push(json!({"role": "assistant", "content": response.content}));

            if tool_uses.is_empty() {
                if stop == StopReason::PauseTurn {
                    continue;
                }
                let answer = response.texts().join("\n");
                return self.finish(report, TurnOutcome::Answered(answer), obs);
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
        let note = format!(
            "[Stopped: the limit of {max} model calls for one request was reached. The last tool results were received but not analysed.]"
        );
        obs.on_event(&AgentEvent::Warning(&note));
        self.history
            .push(json!({"role": "assistant", "content": [{"type": "text", "text": note}]}));
        self.finish(report, TurnOutcome::IterationLimit(max), obs)
    }

    fn finish(
        &mut self,
        mut report: TurnReport,
        outcome: TurnOutcome,
        obs: &dyn Observer,
    ) -> TurnReport {
        self.turn_start = None;
        report.outcome = outcome;
        obs.on_event(&AgentEvent::TurnEnd(&report));
        report
    }

    fn abort(
        &mut self,
        report: TurnReport,
        outcome: TurnOutcome,
        obs: &dyn Observer,
    ) -> TurnReport {
        self.recover_interrupted();
        self.finish(report, outcome, obs)
    }

    /// Runs (or refuses) one tool call and returns its `tool_result` block.
    async fn execute(&self, call: &ToolUse, obs: &dyn Observer) -> Value {
        let started = Instant::now();
        let mut rec = ToolCallRecord {
            tool_use_id: call.id.clone(),
            name: call.name.clone(),
            server: String::new(),
            permission: None,
            input: self.redactor.json(&call.input),
            input_sha256: String::new(),
            executed_input: None,
            status: ToolStatus::Ok,
            error: None,
            approval: ApprovalStatus::NotRequired,
            approved_payload_sha256: None,
            reviewer: None,
            injection_flags: Vec::new(),
            redactions: 0,
            duration_ms: 0,
            output_chars: 0,
            summary: String::new(),
        };

        let (content, is_error) = match self.registry.get(&call.name) {
            None => {
                let known: Vec<&str> = self.registry.iter().map(|t| t.name.as_str()).collect();
                let message = format!(
                    "Unknown or unauthorized tool `{}`. It was not run. Available tools: {}.",
                    call.name,
                    known.join(", ")
                );
                obs.on_event(&AgentEvent::Warning(&message));
                rec.input_sha256 = payload_hash("", &call.name, &call.input);
                rec.status = ToolStatus::UnknownTool;
                rec.error = Some(message.clone());
                (message, true)
            }
            Some(tool) => {
                rec.server = tool.server.to_string();
                rec.permission = Some(tool.permission);
                rec.input_sha256 = payload_hash(tool.server, &tool.name, &call.input);
                obs.on_event(&AgentEvent::ToolStart {
                    name: &tool.name,
                    server: tool.server,
                    permission: tool.permission,
                    input: &rec.input,
                });
                self.run_tool(tool, &call.input, obs, &mut rec).await
            }
        };

        rec.duration_ms = started.elapsed().as_millis();
        rec.output_chars = content.chars().count();
        if rec.summary.is_empty() {
            rec.summary = rec
                .error
                .clone()
                .unwrap_or_else(|| summarise(&content, is_error));
        }
        obs.on_event(&AgentEvent::ToolEnd(&rec));
        tool_result_block(&call.id, &content, is_error)
    }

    async fn run_tool(
        &self,
        tool: &RegisteredTool,
        input: &Value,
        obs: &dyn Observer,
        rec: &mut ToolCallRecord,
    ) -> (String, bool) {
        let problems = validate(input, &tool.mcp_schema);
        if !problems.is_empty() {
            let message = format!(
                "Invalid arguments for {} - the tool was NOT run: {}",
                tool.name,
                problems.join("; ")
            );
            rec.status = ToolStatus::InvalidArguments;
            rec.error = Some(message.clone());
            if tool.permission == Permission::RequiresApproval {
                rec.approval = ApprovalStatus::NotReached;
            }
            return (message, true);
        }
        match tool.permission {
            Permission::Blocked => {
                rec.status = ToolStatus::Blocked;
                let message = format!("{} is blocked by policy and was not run.", tool.name);
                rec.error = Some(message.clone());
                (message, true)
            }
            Permission::ReadOnly => self.call_server(tool, input, rec).await,
            Permission::RequiresApproval => self.run_with_approval(tool, input, obs, rec).await,
        }
    }

    /// Pause → show the exact payload → approve / reject / edit → execute only
    /// the approved payload (verified by hash).
    async fn run_with_approval(
        &self,
        tool: &RegisteredTool,
        input: &Value,
        obs: &dyn Observer,
        rec: &mut ToolCallRecord,
    ) -> (String, bool) {
        let proposal = Proposal::new(
            &tool.name,
            tool.server,
            input.clone(),
            tool.mcp_schema.clone(),
        );
        let decision = self.approver.review(&proposal).await;
        obs.on_event(&AgentEvent::Approval {
            proposal: &proposal,
            decision: &decision,
        });

        match decision {
            Decision::Approved {
                arguments,
                payload_sha256,
                edited,
                reviewer,
            } => {
                rec.reviewer = Some(reviewer);
                rec.approved_payload_sha256 = Some(payload_sha256.clone());
                let actual = payload_hash(tool.server, &tool.name, &arguments);
                if actual != payload_sha256 {
                    rec.approval = ApprovalStatus::PayloadMismatch;
                    rec.status = ToolStatus::NotExecuted;
                    let message =
                        "NOT EXECUTED: the payload to execute does not match the payload that was \
                                   approved (SHA-256 mismatch). Nothing was changed."
                            .to_string();
                    rec.error = Some(message.clone());
                    obs.on_event(&AgentEvent::Warning(&message));
                    return (message, true);
                }
                rec.approval = if edited {
                    ApprovalStatus::ApprovedAfterEdit
                } else {
                    ApprovalStatus::Approved
                };
                let problems = validate(&arguments, &tool.mcp_schema);
                if !problems.is_empty() {
                    rec.status = ToolStatus::InvalidArguments;
                    let message = format!(
                        "The approved payload is invalid - NOT executed: {}",
                        problems.join("; ")
                    );
                    rec.error = Some(message.clone());
                    return (message, true);
                }
                if edited {
                    rec.executed_input = Some(self.redactor.json(&arguments));
                }
                let (content, is_error) = self.call_server(tool, &arguments, rec).await;
                let header = if edited {
                    format!(
                        "A human reviewer EDITED this action and approved the edited payload (SHA-256 {payload_sha256}). \
                         Executed payload: {arguments}\n"
                    )
                } else {
                    format!(
                        "A human reviewer approved this exact payload (SHA-256 {payload_sha256}).\n"
                    )
                };
                (format!("{header}{content}"), is_error)
            }
            Decision::Rejected { reason, reviewer } => {
                rec.reviewer = Some(reviewer);
                rec.approval = ApprovalStatus::Rejected;
                rec.status = ToolStatus::Rejected;
                rec.error = Some(format!("rejected: {reason}"));
                (
                    format!(
                        "NOT EXECUTED: the human reviewer rejected this action. Reason: {reason}. Nothing was \
                         changed. Do not repeat the same request unless the user asks; tell the user it was rejected."
                    ),
                    true,
                )
            }
            Decision::Unavailable(reason) => {
                rec.approval = ApprovalStatus::Unavailable;
                rec.status = ToolStatus::NotExecuted;
                rec.error = Some(reason.clone());
                (
                    format!(
                        "NOT EXECUTED: {reason}. Do not claim the action happened; present it as a proposal \
                         awaiting approval."
                    ),
                    true,
                )
            }
        }
    }

    async fn call_server(
        &self,
        tool: &RegisteredTool,
        input: &Value,
        rec: &mut ToolCallRecord,
    ) -> (String, bool) {
        let Some(conn) = self.servers.get(tool.server) else {
            rec.status = ToolStatus::Failed;
            let message = format!(
                "The {} server is not connected in this session.",
                tool.server
            );
            rec.error = Some(message.clone());
            return (message, true);
        };
        let timeout = self.settings.timeout_for(&tool.name);
        match &tool.target {
            ToolTarget::McpTool(mcp_name) => {
                let arguments = input.as_object().cloned().unwrap_or_default();
                match conn.call_tool(mcp_name, arguments, timeout).await {
                    Ok(result) => self.server_output(
                        tool,
                        render_call_result(&result),
                        result.is_error == Some(true),
                        rec,
                    ),
                    Err(err) => self.mcp_failure(tool, err, timeout, rec),
                }
            }
            ToolTarget::ReadResource { uris, templates } => {
                let uri = input.get("uri").and_then(Value::as_str).unwrap_or_default();
                if !resource_uri_allowed(uri, uris, templates) {
                    let mut valid = uris.clone();
                    valid.extend(templates.iter().cloned());
                    let message = format!(
                        "Unknown resource URI `{uri}` - not requested. Valid: {}.",
                        valid.join(", ")
                    );
                    rec.status = ToolStatus::InvalidArguments;
                    rec.error = Some(message.clone());
                    return (message, true);
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
                        self.server_output(tool, text, false, rec)
                    }
                    Err(err) => self.mcp_failure(tool, err, timeout, rec),
                }
            }
        }
    }

    /// Everything a server returns passes through here: injection scan,
    /// secret redaction, size cap, and the untrusted-data label.
    fn server_output(
        &self,
        tool: &RegisteredTool,
        raw: String,
        is_error: bool,
        rec: &mut ToolCallRecord,
    ) -> (String, bool) {
        let flags = scan(&raw);
        let (clean, redactions) = self.redactor.text(&raw);
        let clean = truncate(clean);
        rec.injection_flags = flags.clone();
        rec.redactions = redactions;
        rec.status = if is_error {
            ToolStatus::ToolError
        } else {
            ToolStatus::Ok
        };
        rec.summary = summarise(&clean, is_error);
        if is_error {
            rec.error = Some(clean.chars().take(300).collect());
        }
        (
            wrap_untrusted(&tool.name, tool.server, &clean, &flags),
            is_error,
        )
    }

    fn mcp_failure(
        &self,
        tool: &RegisteredTool,
        err: McpError,
        timeout: Duration,
        rec: &mut ToolCallRecord,
    ) -> (String, bool) {
        let (status, message) = match err {
            McpError::Rejected { message, data, .. } => {
                let detail = data
                    .as_ref()
                    .and_then(|d| d.get("error"))
                    .map(|d| format!(" {d}"))
                    .unwrap_or_default();
                return self.server_output(tool, format!("{message}{detail}"), true, rec);
            }
            McpError::Timeout { .. } => (
                ToolStatus::Timeout,
                format!(
                    "The tool {} did not finish within {} ms and was abandoned. Its result is unknown - do not \
                     assume it succeeded.",
                    tool.name,
                    timeout.as_millis()
                ),
            ),
            McpError::Disconnected { server, .. } => (
                ToolStatus::Disconnected,
                format!(
                    "The {server} server has disconnected; its tools are unavailable for the rest of this session."
                ),
            ),
            other => (ToolStatus::Failed, other.to_string()),
        };
        rec.status = status;
        rec.error = Some(message.clone());
        (message, true)
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

fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!("{cut}\n[... truncated: the result was longer than {MAX_RESULT_CHARS} characters]")
}

/// One short line for the console.
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
        let out = truncate("x".repeat(MAX_RESULT_CHARS + 10));
        assert!(out.ends_with("characters]"));
        assert!(out.chars().count() < MAX_RESULT_CHARS + 100);
    }

    #[test]
    fn per_tool_timeouts_override_the_default() {
        let mut s = AgentSettings {
            model: "m".into(),
            effort: Effort::Medium,
            fallbacks: false,
            progress_updates: false,
            max_iterations: 8,
            max_tokens: 1,
            tool_timeout: Duration::from_secs(10),
            tool_timeouts: BTreeMap::new(),
            secrets: vec![],
            path_aliases: vec![],
        };
        s.tool_timeouts
            .insert("files__search_files".into(), Duration::from_secs(30));
        assert_eq!(
            s.timeout_for("files__search_files"),
            Duration::from_secs(30)
        );
        assert_eq!(
            s.timeout_for("telemetry__search_logs"),
            Duration::from_secs(10)
        );
    }
}
