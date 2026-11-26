//! The command-line interface: an interactive session (`agent-client`) and a
//! one-shot mode (`agent-client ask "..."`). Both print the agent's progress
//! as it happens (each model call, every tool call with its outcome, progress
//! notes, security warnings), ask a human before any state-changing action,
//! and write every step to the JSONL trace file.

use std::io::Write;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::agent::approval::{ConsoleApprover, Decision, LineSource, SharedStdin};
use crate::agent::registry::Permission;
use crate::agent::{
    Agent, AgentEvent, AgentSettings, ApprovalStatus, Fanout, Observer, ToolStatus, TurnOutcome,
    TurnReport,
};
use crate::config::{Config, Provider};
use crate::llm::types::MAX_TOKENS;
use crate::llm::{AnthropicClient, AnyClient, OpenRouterClient, StopReason};
use crate::mcp::manager::ServerSet;
use crate::safety::redact::Redactor;
use crate::trace::{Tracer, new_session_id};

const HELP: &str = "Type an investigation request, e.g. \"Investigate the payments-api errors\".
Commands: /tools  list discovered tools    /servers  show MCP servers
          /reset  forget the conversation  /help     this text
          /exit   quit (also /quit, Ctrl+Z then Enter, or Ctrl+C at the prompt)
Ctrl+C while the agent is working cancels that request.
State-changing actions (incident tickets) always stop and ask you first.";

/// Prints agent events to the console.
pub struct ConsoleObserver {
    pub show_tokens: bool,
    pub redactor: Redactor,
}

impl Observer for ConsoleObserver {
    fn on_event(&self, event: &AgentEvent<'_>) {
        match event {
            AgentEvent::ModelCall { iteration, max } => {
                println!("  [step {iteration}/{max}] asking Claude...")
            }
            AgentEvent::ModelReply {
                stop,
                input_tokens,
                output_tokens,
                cache_read,
                millis,
            } => {
                if self.show_tokens {
                    println!(
                        "    (stop: {}, {millis} ms, tokens in {input_tokens} / cached {cache_read} / out {output_tokens})",
                        stop_label(stop)
                    );
                }
            }
            AgentEvent::Note(note) => {
                for line in note.lines().filter(|l| !l.trim().is_empty()) {
                    println!("  ~ {}", line.trim());
                }
            }
            AgentEvent::Text(text) => println!("\nclaude> {}\n", text.trim()),
            AgentEvent::Fallback(switch) => println!("  ! refusal fallback: {switch}"),
            AgentEvent::ToolStart {
                name,
                permission,
                input,
                ..
            } => {
                let tag = match permission {
                    Permission::ReadOnly => "",
                    Permission::RequiresApproval => "  [STATE-CHANGING: needs your approval]",
                    Permission::Blocked => "  [blocked]",
                };
                println!(
                    "  -> {name} {}{tag}",
                    compact(&self.redactor.json(input), 160)
                );
            }
            AgentEvent::Approval { decision, .. } => {
                let line = match decision {
                    Decision::Approved { edited: false, .. } => {
                        "  >> APPROVED - executing exactly the payload shown".to_string()
                    }
                    Decision::Approved { edited: true, .. } => {
                        "  >> EDITED and APPROVED - executing the edited payload".to_string()
                    }
                    Decision::Rejected { reason, .. } => {
                        format!("  >> REJECTED ({reason}) - nothing was executed")
                    }
                    Decision::Unavailable(r) => format!("  >> NOT EXECUTED: {r}"),
                };
                println!("{line}");
            }
            AgentEvent::ToolEnd(rec) => {
                let mark = match rec.status {
                    ToolStatus::Ok => "ok",
                    ToolStatus::Rejected | ToolStatus::NotExecuted => "--",
                    _ => "!!",
                };
                let mut line = format!(
                    "     {mark} {}: {} ({} ms)",
                    rec.name,
                    self.redactor.redact_str(&rec.summary),
                    rec.duration_ms
                );
                if rec.status != ToolStatus::Ok && rec.status != ToolStatus::ToolError {
                    line.push_str(&format!(" [{}]", rec.status.as_str()));
                }
                if rec.approval == ApprovalStatus::PayloadMismatch {
                    line.push_str(" [payload changed after approval - blocked]");
                }
                println!("{line}");
                if !rec.injection_flags.is_empty() {
                    println!(
                        "     !! SECURITY: instruction-like text in this output ({}). Labelled as untrusted data for Claude.",
                        rec.injection_flags.join(", ")
                    );
                }
                if rec.redactions > 0 {
                    println!(
                        "     !! {} secret-looking value(s) redacted from this output",
                        rec.redactions
                    );
                }
            }
            AgentEvent::Warning(w) => println!("  ! {}", self.redactor.redact_str(w)),
            AgentEvent::TurnStart { .. }
            | AgentEvent::TurnEnd(_)
            | AgentEvent::ModelError { .. } => {}
        }
        let _ = std::io::stdout().flush();
    }
}

fn stop_label(stop: &StopReason) -> String {
    match stop {
        StopReason::EndTurn => "end_turn".into(),
        StopReason::ToolUse => "tool_use".into(),
        StopReason::MaxTokens => "max_tokens".into(),
        StopReason::Refusal(c) => format!("refusal ({})", c.as_deref().unwrap_or("?")),
        StopReason::PauseTurn => "pause_turn".into(),
        StopReason::StopSequence => "stop_sequence".into(),
        StopReason::Other(o) => o.clone(),
        StopReason::Missing => "none".into(),
    }
}

fn compact(value: &Value, max: usize) -> String {
    let s = value.to_string();
    if s.chars().count() > max {
        format!("{}...", s.chars().take(max).collect::<String>())
    } else {
        s
    }
}

pub fn agent_settings(config: &Config) -> AgentSettings {
    AgentSettings {
        model: config.model.clone(),
        effort: config.effort,
        fallbacks: config.fallbacks,
        progress_updates: config.progress_updates,
        max_iterations: config.max_iterations,
        max_tokens: MAX_TOKENS,
        tool_timeout: config.mcp.tool_timeout,
        tool_timeouts: config.tool_timeout_overrides.clone(),
        secrets: vec![config.api_key.expose().to_string()],
        path_aliases: path_aliases(config),
    }
}

/// The project folder and the user's home folder, shown as placeholders.
fn path_aliases(config: &Config) -> Vec<(String, String)> {
    let mut aliases = Vec::new();
    if let Ok(root) = crate::mcp::filesystem::canonical(&config.mcp.project_root) {
        aliases.push((root.display().to_string(), "<project>".to_string()));
    }
    aliases.push((
        config.mcp.project_root.display().to_string(),
        "<project>".to_string(),
    ));
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        aliases.push((home.to_string_lossy().into_owned(), "<home>".to_string()));
    }
    aliases
}

/// A running session: the agent plus its trace file.
pub struct Session {
    pub agent: Agent<AnyClient>,
    pub tracer: Tracer,
    pub console: ConsoleObserver,
}

impl Session {
    pub async fn run(&mut self, request: &str) -> TurnReport {
        let observers = Fanout(vec![&self.console, &self.tracer]);
        self.agent.run_turn(request, &observers).await
    }

    pub async fn close(self) {
        self.tracer.session_end();
        self.agent.shutdown().await;
    }
}

/// Connects both MCP servers, discovers tools, opens the trace and builds the agent.
pub async fn start_session(
    config: &Config,
    input: SharedStdin,
    show_tokens: bool,
) -> Result<Session> {
    let redactor = agent_settings(config).redactor();
    let tracer = Tracer::open(&config.trace_file, new_session_id(), redactor.clone())
        .with_context(|| {
            format!(
                "could not open the trace file {}",
                config.trace_file.display()
            )
        })?;

    println!("Starting MCP servers...");
    let servers = ServerSet::connect_all(&config.mcp).await;
    for s in servers.discovered() {
        println!(
            "  [ok] {}: {} | {} tools, {} resources, {} templates",
            s.name,
            s.label,
            s.tools.len(),
            s.resources.len(),
            s.templates.len()
        );
    }
    for (name, reason) in servers.unavailable() {
        println!("  [!!] {name}: unavailable - {reason}");
    }
    if servers.is_empty() {
        tracer.record(
            "session_start",
            json!({"error": "no MCP server could be started"}),
        );
        servers.shutdown().await;
        bail!("no MCP server could be started; run `agent-client check-mcp` for details");
    }

    let llm = match config.provider {
        Provider::Anthropic => AnyClient::Anthropic(
            AnthropicClient::new(
                config.api_key.clone(),
                &config.api_base_url,
                config.request_timeout,
            )
            .context("could not create the Claude API client")?,
        ),
        Provider::OpenRouter => AnyClient::OpenRouter(
            OpenRouterClient::new(
                config.api_key.clone(),
                config.model.clone(),
                Some(config.api_base_url.clone()),
                config.openrouter_fallbacks.clone(),
                config.request_timeout,
            )
            .context("could not create the OpenRouter client")?,
        ),
    };
    let agent = Agent::new(
        llm,
        agent_settings(config),
        servers,
        &config.mcp.ops_reference_time,
        Box::new(ConsoleApprover::new(input)),
    );
    let reg = agent.registry();
    let read_only = reg
        .iter()
        .filter(|t| t.permission == Permission::ReadOnly)
        .count();
    let approval = reg
        .iter()
        .filter(|t| t.permission == Permission::RequiresApproval)
        .count();
    // `effort` is an Anthropic-only setting; do not show it for other providers.
    let effort = if config.provider == Provider::Anthropic {
        format!(", effort {}", config.effort)
    } else {
        String::new()
    };
    println!(
        "Discovered {} tools for Claude ({read_only} read-only, {approval} need approval; {} excluded by policy). Provider: {}, model: {}{effort}, max {} steps per request.",
        reg.len(),
        reg.excluded.len(),
        config.provider,
        config.model,
        config.max_iterations
    );
    println!(
        "Trace file: {} (session {})",
        redactor.redact_str(&tracer.path().display().to_string()),
        tracer.session_id()
    );

    tracer.session_start(json!({
        "provider": config.provider.to_string(),
        "model": config.model,
        // Which Claude endpoint served this session (the real API, or a local test endpoint).
        "api_endpoint": config.api_base_url,
        "agent_version": env!("CARGO_PKG_VERSION"),
        "effort": config.effort.to_string(),
        "max_iterations": config.max_iterations,
        "tool_timeout_ms": config.mcp.tool_timeout.as_millis() as u64,
        "servers": agent.servers().discovered().iter().map(|s| json!({"name": s.name, "label": s.label})).collect::<Vec<_>>(),
        "unavailable": agent.servers().unavailable().iter().map(|(n, r)| json!({"name": n, "reason": r})).collect::<Vec<_>>(),
        "tools": reg.iter().map(|t| json!({"name": t.name, "server": t.server, "permission": t.permission.to_string()})).collect::<Vec<_>>(),
        "excluded": reg.excluded.iter().map(|(n, r)| json!({"name": n, "reason": r})).collect::<Vec<_>>(),
    }));
    Ok(Session {
        agent,
        tracer,
        console: ConsoleObserver {
            show_tokens,
            redactor,
        },
    })
}

fn print_tools(agent: &Agent<AnyClient>) {
    for t in agent.registry().iter() {
        println!("  {:<40} {:<10} {}", t.name, t.server, t.permission);
    }
    for (name, reason) in &agent.registry().excluded {
        println!("  {name:<40} excluded: {reason}");
    }
}

fn print_servers(agent: &Agent<AnyClient>) {
    for s in agent.servers().discovered() {
        println!("  {}: {}", s.name, s.label);
    }
    for (name, reason) in agent.servers().unavailable() {
        println!("  {name}: unavailable - {reason}");
    }
}

fn print_report(report: &TurnReport) {
    let stats = format!(
        "{} model call(s), {} tool call(s), {} input / {} output tokens",
        report.model_calls, report.tool_calls, report.input_tokens, report.output_tokens
    );
    match &report.outcome {
        TurnOutcome::Answered(_) => println!("  [done: {stats}]"),
        TurnOutcome::Refused(category) => println!(
            "  [Claude declined this request{}; nothing was executed from that reply. Rephrase and try again. {stats}]",
            category
                .as_deref()
                .map(|c| format!(" (category: {c})"))
                .unwrap_or_default()
        ),
        TurnOutcome::Truncated => {
            println!(
                "  [the reply was cut off at the output limit; its tool calls were not run. Ask a narrower question. {stats}]"
            )
        }
        TurnOutcome::IterationLimit(max) => {
            println!(
                "  [stopped safely after {max} model calls (MAX_ITERATIONS). Ask a follow-up to continue. {stats}]"
            )
        }
        TurnOutcome::Failed(error) => {
            println!("  [error: {error}. The request was rolled back; you can retry. {stats}]")
        }
    }
}

/// `agent-client`: the interactive session.
pub async fn interactive(config: &Config) -> Result<()> {
    println!("Incident Triage & Release Readiness Agent");
    let input = SharedStdin::new();
    let mut session = start_session(config, input.clone(), false).await?;
    println!("\n{HELP}");

    loop {
        print!("\nyou> ");
        let _ = std::io::stdout().flush();
        let line = tokio::select! {
            line = input.read_line() => line,
            _ = tokio::signal::ctrl_c() => { println!(); break; }
        };
        let Some(line) = line else { break }; // end of input
        match line.trim() {
            "" => continue,
            "/exit" | "/quit" => break,
            "/help" => println!("{HELP}"),
            "/tools" => print_tools(&session.agent),
            "/servers" => print_servers(&session.agent),
            "/reset" => {
                session.agent.reset();
                println!("Conversation cleared.");
            }
            cmd if cmd.starts_with('/') => println!("Unknown command {cmd}. Type /help."),
            request => {
                let report = tokio::select! {
                    report = session.run(request) => Some(report),
                    _ = tokio::signal::ctrl_c() => None,
                };
                match report {
                    Some(report) => print_report(&report),
                    None => {
                        session.agent.recover_interrupted();
                        session.tracer.record(
                            "warning",
                            json!({"message": "request cancelled by the user (Ctrl+C)"}),
                        );
                        println!("\n  [cancelled; the partial request was discarded]");
                    }
                }
            }
        }
    }
    println!("Shutting down MCP servers...");
    session.close().await;
    println!("Bye.");
    Ok(())
}

/// `agent-client ask "..."`: one request, then exit. Returns true if answered.
/// Approval questions are still asked on the console.
pub async fn ask(config: &Config, question: &str) -> Result<bool> {
    let mut session = start_session(config, SharedStdin::new(), true).await?;
    println!("\nyou> {question}");
    let report = session.run(question).await;
    print_report(&report);
    session.close().await;
    Ok(matches!(report.outcome, TurnOutcome::Answered(_)))
}
