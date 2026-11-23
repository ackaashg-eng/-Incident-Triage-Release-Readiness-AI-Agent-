//! The command-line interface: an interactive session (`agent-client`) and a
//! one-shot mode (`agent-client ask "..."`). Both print the agent's progress
//! as it happens: each model call, every tool call with its arguments and
//! outcome, progress notes, proposals, and the final answer.

use std::io::Write;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::agent::approval::{Decision, DeferredApprover};
use crate::agent::registry::Permission;
use crate::agent::{Agent, AgentEvent, AgentSettings, Observer, TurnOutcome, TurnReport};
use crate::config::Config;
use crate::llm::types::MAX_TOKENS;
use crate::llm::{AnthropicClient, StopReason};
use crate::mcp::manager::ServerSet;

const HELP: &str = "Type an investigation request, e.g. \"Investigate the payments-api errors\".
Commands: /tools  list discovered tools    /servers  show MCP servers
          /reset  forget the conversation  /help     this text
          /exit   quit (also /quit, Ctrl+Z then Enter, or Ctrl+C at the prompt)
Ctrl+C while the agent is working cancels that request.";

/// Prints agent events to the console.
pub struct ConsoleObserver {
    pub show_tokens: bool,
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
            } => {
                if self.show_tokens {
                    println!(
                        "    (stop: {}, tokens in {input_tokens} / cached {cache_read} / out {output_tokens})",
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
                    Permission::RequiresApproval => "  [needs approval]",
                    Permission::Blocked => "  [blocked]",
                };
                println!("  -> {name} {}{tag}", compact(input, 160));
            }
            AgentEvent::ToolEnd {
                name,
                ok,
                summary,
                millis,
            } => {
                println!(
                    "     {} {name}: {summary} ({millis} ms)",
                    if *ok { "ok" } else { "!!" }
                );
            }
            AgentEvent::Proposal { proposal, decision } => {
                println!("\n  ==== PROPOSED ACTION: {} ====", proposal.tool);
                let pretty = serde_json::to_string_pretty(&proposal.arguments).unwrap_or_default();
                for line in pretty.lines() {
                    println!("  | {line}");
                }
                let status = match decision {
                    Decision::Approved(_) => "approved - executing".to_string(),
                    Decision::Rejected(r) => format!("rejected: {r}"),
                    Decision::Unavailable(r) => format!("NOT executed: {r}"),
                };
                println!("  ==== {status} ====\n");
            }
            AgentEvent::Warning(w) => println!("  ! {w}"),
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

fn agent_settings(config: &Config) -> AgentSettings {
    AgentSettings {
        model: config.model.clone(),
        effort: config.effort,
        fallbacks: config.fallbacks,
        progress_updates: config.progress_updates,
        max_iterations: config.max_iterations,
        max_tokens: MAX_TOKENS,
        tool_timeout: config.mcp.tool_timeout,
    }
}

/// Connects both MCP servers, discovers tools and builds the agent.
pub async fn start_agent(config: &Config) -> Result<Agent<AnthropicClient>> {
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
        servers.shutdown().await;
        bail!("no MCP server could be started; run `agent-client check-mcp` for details");
    }

    let llm = AnthropicClient::new(
        config.api_key.clone(),
        &config.api_base_url,
        config.request_timeout,
    )
    .context("could not create the Claude API client")?;
    let agent = Agent::new(
        llm,
        agent_settings(config),
        servers,
        &config.mcp.ops_reference_time,
        Box::new(DeferredApprover),
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
    println!(
        "Discovered {} tools for Claude ({read_only} read-only, {approval} need approval; {} excluded by policy). Model: {}, effort {}.",
        reg.len(),
        reg.excluded.len(),
        config.model,
        config.effort
    );
    Ok(agent)
}

fn print_tools(agent: &Agent<AnthropicClient>) {
    for t in agent.registry().iter() {
        println!("  {:<40} {:<10} {}", t.name, t.server, t.permission);
    }
    for (name, reason) in &agent.registry().excluded {
        println!("  {name:<40} excluded: {reason}");
    }
}

fn print_servers(agent: &Agent<AnthropicClient>) {
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
                "  [stopped after {max} model calls (MAX_ITERATIONS). Ask a follow-up to continue. {stats}]"
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
    let mut agent = start_agent(config).await?;
    let observer = ConsoleObserver { show_tokens: false };
    println!("\n{HELP}");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print!("\nyou> ");
        let _ = std::io::stdout().flush();
        let line = tokio::select! {
            line = lines.next_line() => line,
            _ = tokio::signal::ctrl_c() => { println!(); break; }
        };
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => break, // end of input
            Err(err) => {
                eprintln!("could not read input: {err}");
                break;
            }
        };
        let input = line.trim();
        match input {
            "" => continue,
            "/exit" | "/quit" => break,
            "/help" => println!("{HELP}"),
            "/tools" => print_tools(&agent),
            "/servers" => print_servers(&agent),
            "/reset" => {
                agent.reset();
                println!("Conversation cleared.");
            }
            cmd if cmd.starts_with('/') => println!("Unknown command {cmd}. Type /help."),
            request => {
                let report = tokio::select! {
                    report = agent.run_turn(request, &observer) => Some(report),
                    _ = tokio::signal::ctrl_c() => None,
                };
                match report {
                    Some(report) => print_report(&report),
                    None => {
                        agent.recover_interrupted();
                        println!("\n  [cancelled; the partial request was discarded]");
                    }
                }
            }
        }
    }
    println!("Shutting down MCP servers...");
    agent.shutdown().await;
    println!("Bye.");
    Ok(())
}

/// `agent-client ask "..."`: one request, then exit. Returns true if answered.
pub async fn ask(config: &Config, question: &str) -> Result<bool> {
    let mut agent = start_agent(config).await?;
    println!("\nyou> {question}");
    let report = agent
        .run_turn(question, &ConsoleObserver { show_tokens: true })
        .await;
    print_report(&report);
    agent.shutdown().await;
    Ok(matches!(report.outcome, TurnOutcome::Answered(_)))
}
