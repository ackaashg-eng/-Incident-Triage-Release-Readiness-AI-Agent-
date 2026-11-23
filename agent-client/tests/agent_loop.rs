//! The agentic loop with a scripted Claude and the REAL MCP servers.
//!
//! The scripted `LlmClient` replays fixed Claude responses (tool_use blocks
//! like the real API sends) and records every request the agent makes, so we
//! can check what Claude would receive: tool definitions, tool results,
//! history shape.

#![allow(clippy::unwrap_used)]

mod common;

use agent_client::agent::approval::DeferredApprover;
use agent_client::agent::{Agent, NoopObserver, TurnOutcome};
use agent_client::llm::{LlmError, MessagesRequest};
use common::{Recorder, ScriptedLlm, build_agent, message, tool_use};
use serde_json::{Value, json};

async fn agent(
    llm: ScriptedLlm,
    tickets: &std::path::Path,
    max_iterations: u32,
) -> Agent<ScriptedLlm> {
    build_agent(llm, tickets, max_iterations, Box::new(DeferredApprover)).await
}
/// The tool_result blocks of a request's last message, by tool_use_id.
fn results(request: &MessagesRequest) -> Vec<(String, String, bool)> {
    let last = request.messages.last().unwrap();
    assert_eq!(last["role"], "user");
    last["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            assert_eq!(b["type"], "tool_result");
            (
                b["tool_use_id"].as_str().unwrap().to_string(),
                b["content"].as_str().unwrap().to_string(),
                b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            )
        })
        .collect()
}

#[tokio::test]
async fn both_servers_connect_and_tools_are_discovered_and_mapped() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = agent(ScriptedLlm::default(), &tmp.path().join("t.db"), 8).await;
    let reg = agent.registry();
    let server_of = |n: &str| reg.get(n).unwrap().server;
    assert_eq!(server_of("telemetry__search_logs"), "telemetry");
    assert_eq!(server_of("telemetry__get_deployment_history"), "telemetry");
    assert_eq!(server_of("telemetry__read_resource"), "telemetry");
    assert_eq!(server_of("files__read_text_file"), "files");
    assert_eq!(server_of("files__list_directory"), "files");
    assert_eq!(
        reg.len(),
        3 + 1 + 10,
        "3 telemetry tools + read_resource + 10 filesystem tools"
    );
    assert!(reg.excluded.is_empty(), "{:?}", reg.excluded);
    let labels: Vec<&str> = agent
        .servers()
        .discovered()
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    assert!(
        labels[0].starts_with("ops-telemetry-mcp 0.1.0") && labels[0].contains("tools, resources"),
        "{labels:?}"
    );
    assert!(
        labels[1].starts_with("secure-filesystem-server 2.4.0"),
        "{labels:?}"
    );
    agent.shutdown().await;
}

/// The Phase 5 example: logs → deployments → runbook (+ release notes) →
/// assessment → ticket proposal, including two tool calls in ONE response.
#[tokio::test]
async fn a_multi_step_investigation_runs_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let tickets = tmp.path().join("tickets.db");
    let llm = ScriptedLlm::new(vec![
        Ok(message(
            "tool_use",
            json!([
                {"type": "thinking", "thinking": "Start with recent errors and deployments.", "signature": "sig-1"},
                tool_use("t1", "telemetry__search_logs", json!({"service_name": "payments-api", "severity": "ERROR", "limit": 10})),
                tool_use("t2", "telemetry__get_deployment_history", json!({"service_name": "payments-api", "window_hours": 24}))
            ]),
        )),
        Ok(message(
            "tool_use",
            json!([
                {"type": "text", "text": "Errors start right after 2.14.0. Checking the runbook and release notes."},
                tool_use("t3", "telemetry__read_resource", json!({"uri": "service://runbooks/payments-api"})),
                tool_use("t4", "files__read_text_file", json!({"path": "RELEASE_NOTES.md"}))
            ]),
        )),
        Ok(message(
            "tool_use",
            json!([tool_use(
                "t5",
                "telemetry__create_incident_ticket",
                json!({
                    "title": "payments-api checkout failures after 2.14.0",
                    "severity": "CRITICAL",
                    "summary": "Gateway timeouts since 09:12 UTC after deploy dep-1048 (2.14.0); circuit breaker open.",
                    "assignee": "payments-oncall"
                })
            )]),
        )),
        Ok(message(
            "end_turn",
            json!([{"type": "text", "text": "Summary: payments-api outage after 2.14.0. Ticket proposed."}]),
        )),
    ]);
    let mut agent = agent(llm.clone(), &tickets, 8).await;
    let recorder = Recorder::default();
    let report = agent
        .run_turn("Investigate the payment API errors.", &recorder)
        .await;

    assert_eq!(
        report.outcome,
        TurnOutcome::Answered("Summary: payments-api outage after 2.14.0. Ticket proposed.".into())
    );
    assert_eq!((report.model_calls, report.tool_calls), (4, 5));

    let requests = llm.requests();
    assert_eq!(requests.len(), 4);

    // Claude received valid tool definitions, identical in every request.
    let tools = &requests[0].tools;
    assert!(
        tools
            .iter()
            .all(|t| t.input_schema["type"] == "object" && t.input_schema.get("$schema").is_none())
    );
    assert!(
        tools.windows(2).all(|w| w[0].name < w[1].name),
        "sorted by name"
    );
    for r in &requests[1..] {
        assert_eq!(&r.tools, tools, "the tool list must not change mid-session");
        assert_eq!(
            r.system, requests[0].system,
            "the system prompt must not change mid-session"
        );
    }
    assert!(!tools.iter().any(|t| t.name.contains("write_file")));

    // Request 2 carries BOTH results of the parallel calls, in order.
    let r2 = results(&requests[1]);
    assert_eq!(
        r2.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["t1", "t2"]
    );
    assert!(
        r2[0].1.contains("log-0029") && r2[0].1.contains("GATEWAY_TIMEOUT"),
        "{}",
        r2[0].1
    );
    assert!(
        r2[1].1.contains("2.14.0") && r2[1].1.contains("561378f2a66e81041065b3d62027e7ec2f074573")
    );
    assert!(!r2[0].2 && !r2[1].2);

    // Request 3: runbook (resource) + release notes (filesystem server).
    let r3 = results(&requests[2]);
    assert!(r3[0].1.contains("# Runbook: payments-api"), "{}", r3[0].1);
    assert!(
        r3[1].1.contains("2.14.0")
            && r3[1].1.contains("Load test: **skipped**")
            && r3[1]
                .1
                .contains("<tool_output tool=\"files__read_text_file\""),
        "{}",
        r3[1].1
    );

    // Request 4: the ticket was proposed, NOT executed.
    let r4 = results(&requests[3]);
    assert!(r4[0].2, "reported as not executed");
    assert!(r4[0].1.contains("NOT EXECUTED"), "{}", r4[0].1);
    assert!(!tickets.exists(), "no ticket database was even created");

    // History: verbatim assistant content (thinking signature kept), alternating roles.
    let history = agent.history();
    assert_eq!(history.len(), 8);
    let roles: Vec<&str> = history
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        [
            "user",
            "assistant",
            "user",
            "assistant",
            "user",
            "assistant",
            "user",
            "assistant"
        ]
    );
    assert_eq!(history[1]["content"][0]["signature"], "sig-1");

    let events = recorder.0.lock().unwrap().clone();
    assert!(events.contains(&"note Start with recent errors and deployments.".to_string()));
    assert!(events.contains(&"proposal telemetry__create_incident_ticket".to_string()));
    assert!(events.contains(&"end files__read_text_file true".to_string()));
    agent.shutdown().await;
}

#[tokio::test]
async fn unknown_tools_invalid_arguments_and_tool_failures_go_back_to_claude() {
    let tmp = tempfile::tempdir().unwrap();
    let llm = ScriptedLlm::new(vec![
        Ok(message(
            "tool_use",
            json!([
                tool_use("a", "telemetry__delete_logs", json!({})),
                tool_use(
                    "b",
                    "telemetry__search_logs",
                    json!({"service_name": "payments-api", "limit": "lots"})
                ),
                tool_use(
                    "c",
                    "telemetry__search_logs",
                    json!({"service_name": "billing-service"})
                ),
                tool_use(
                    "d",
                    "files__read_text_file",
                    json!({"path": "..\\..\\.env"})
                ),
                tool_use(
                    "e",
                    "telemetry__read_resource",
                    json!({"uri": "service://runbooks/../../.env"})
                ),
                tool_use(
                    "f",
                    "telemetry__read_resource",
                    json!({"uri": "service://runbooks/billing-service"})
                ),
                tool_use(
                    "g",
                    "telemetry__get_deployment_history",
                    json!({"service_name": "auth-service", "window_hours": -5})
                )
            ]),
        )),
        Ok(message(
            "end_turn",
            json!([{"type": "text", "text": "Handled."}]),
        )),
    ]);
    let mut agent = agent(llm.clone(), &tmp.path().join("t.db"), 8).await;
    let report = agent.run_turn("try things", &NoopObserver).await;
    assert_eq!(report.outcome, TurnOutcome::Answered("Handled.".into()));

    let r = results(&llm.requests()[1]);
    assert_eq!(r.len(), 7, "every tool_use gets exactly one result");
    assert!(r.iter().all(|(_, _, is_error)| *is_error), "{r:#?}");
    let text = |i: usize| r[i].1.as_str();
    assert!(
        text(0).contains("Unknown or unauthorized tool `telemetry__delete_logs`"),
        "{}",
        text(0)
    );
    assert!(
        text(1).contains("NOT run") && text(1).contains("arguments.limit"),
        "{}",
        text(1)
    );
    assert!(text(2).contains("UNKNOWN_SERVICE"), "{}", text(2));
    assert!(text(3).contains("Access denied"), "{}", text(3));
    assert!(text(4).contains("Unknown resource URI"), "{}", text(4));
    assert!(
        text(5).contains("UNKNOWN_SERVICE"),
        "server-side missing resource: {}",
        text(5)
    );
    assert!(
        text(6).contains("NOT run") && text(6).contains("minimum"),
        "client-side validation: {}",
        text(6)
    );
    agent.shutdown().await;
}

#[tokio::test]
async fn refusals_and_truncation_never_run_tools_and_roll_back() {
    let tmp = tempfile::tempdir().unwrap();
    let llm = ScriptedLlm::new(vec![
        Ok(
            json!({"content": [tool_use("x", "telemetry__search_logs", json!({"service_name": "payments-api"}))],
                  "stop_reason": "refusal", "stop_details": {"category": "cyber"}, "usage": {}}),
        ),
        Ok(message(
            "max_tokens",
            json!([tool_use(
                "y",
                "telemetry__search_logs",
                json!({"service_name": "pay"})
            )]),
        )),
        Ok(message(
            "end_turn",
            json!([{"type": "text", "text": "fine"}]),
        )),
    ]);
    let mut agent = agent(llm.clone(), &tmp.path().join("t.db"), 8).await;
    let recorder = Recorder::default();

    let r1 = agent.run_turn("first", &recorder).await;
    assert_eq!(r1.outcome, TurnOutcome::Refused(Some("cyber".into())));
    assert!(agent.history().is_empty(), "rolled back");

    let r2 = agent.run_turn("second", &recorder).await;
    assert_eq!(r2.outcome, TurnOutcome::Truncated);
    assert!(agent.history().is_empty());
    assert!(
        recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|e| !e.starts_with("start")),
        "no tool ran"
    );

    // The next turn starts from a clean history.
    let r3 = agent.run_turn("third", &recorder).await;
    assert_eq!(r3.outcome, TurnOutcome::Answered("fine".into()));
    assert_eq!(llm.requests()[2].messages.len(), 1);
    agent.shutdown().await;
}

#[tokio::test]
async fn api_failures_roll_back_and_the_session_continues() {
    let tmp = tempfile::tempdir().unwrap();
    let llm = ScriptedLlm::new(vec![
        Ok(message(
            "tool_use",
            json!([tool_use(
                "a",
                "telemetry__search_logs",
                json!({"service_name": "auth-service", "limit": 1})
            )]),
        )),
        Err(LlmError::Overloaded { attempts: 4 }),
        Ok(message(
            "end_turn",
            json!([{"type": "text", "text": "back"}]),
        )),
    ]);
    let mut agent = agent(llm.clone(), &tmp.path().join("t.db"), 8).await;
    let r = agent.run_turn("one", &NoopObserver).await;
    assert!(
        matches!(&r.outcome, TurnOutcome::Failed(e) if e.contains("overloaded")),
        "{:?}",
        r.outcome
    );
    assert!(agent.history().is_empty(), "partial turn discarded");
    let r = agent.run_turn("two", &NoopObserver).await;
    assert_eq!(r.outcome, TurnOutcome::Answered("back".into()));
    agent.shutdown().await;
}

#[tokio::test]
async fn the_iteration_limit_stops_a_looping_model() {
    let tmp = tempfile::tempdir().unwrap();
    let call = || {
        Ok(message(
            "tool_use",
            json!([tool_use("a", "files__list_directory", json!({"path": "."}))]),
        ))
    };
    let llm = ScriptedLlm::new(vec![
        call(),
        call(),
        call(),
        Ok(message(
            "end_turn",
            json!([{"type": "text", "text": "after"}]),
        )),
    ]);
    let mut agent = agent(llm.clone(), &tmp.path().join("t.db"), 3).await;

    let r = agent.run_turn("loop forever", &NoopObserver).await;
    assert_eq!(r.outcome, TurnOutcome::IterationLimit(3));
    assert_eq!(r.model_calls, 3);
    let last = agent.history().last().unwrap();
    assert_eq!(
        last["role"], "assistant",
        "history stays valid for the next turn"
    );

    let r = agent.run_turn("continue", &NoopObserver).await;
    assert_eq!(r.outcome, TurnOutcome::Answered("after".into()));
    let msgs = &llm.requests()[3].messages;
    assert_eq!(msgs.last().unwrap()["role"], "user");
    assert_eq!(msgs[msgs.len() - 2]["role"], "assistant");
    agent.shutdown().await;
}

#[tokio::test]
async fn malformed_model_responses_are_handled() {
    let tmp = tempfile::tempdir().unwrap();
    let llm = ScriptedLlm::new(vec![
        Ok(message(
            "tool_use",
            json!([{"type": "tool_use", "input": {}}]),
        )),
        Ok(message("end_turn", json!([]))),
    ]);
    let mut agent = agent(llm, &tmp.path().join("t.db"), 8).await;
    let r = agent.run_turn("x", &NoopObserver).await;
    assert!(
        matches!(&r.outcome, TurnOutcome::Failed(e) if e.contains("malformed")),
        "{:?}",
        r.outcome
    );
    assert!(agent.history().is_empty());
    let r = agent.run_turn("y", &NoopObserver).await;
    assert_eq!(
        r.outcome,
        TurnOutcome::Answered(String::new()),
        "empty reply is tolerated"
    );
    assert_eq!(agent.history().last().unwrap()["role"], "assistant");
    agent.shutdown().await;
}

#[tokio::test]
async fn reset_clears_the_conversation() {
    let tmp = tempfile::tempdir().unwrap();
    let llm = ScriptedLlm::new(vec![Ok(message(
        "end_turn",
        json!([{"type": "text", "text": "hi"}]),
    ))]);
    let mut agent = agent(llm, &tmp.path().join("t.db"), 8).await;
    agent.run_turn("hello", &NoopObserver).await;
    assert_eq!(agent.history().len(), 2);
    agent.reset();
    assert!(agent.history().is_empty());
    agent.shutdown().await;
}
