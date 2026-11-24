//! End-to-end: the real `agent-client` binary, the real MCP servers, and a
//! local fake Anthropic API (so no key and no network are needed). This is
//! the same path a live run takes, except the HTTP endpoint.

#![allow(clippy::unwrap_used)]

mod common;

use std::process::Stdio;

use common::{FakeAnthropic, Reply, message, project_root};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

fn agent_command(fake: &FakeAnthropic, tickets: &std::path::Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-client"));
    cmd.current_dir(project_root())
        .env("ANTHROPIC_API_KEY", "sk-ant-test-SECRET123")
        .env("ANTHROPIC_BASE_URL", &fake.url)
        .env("TICKETS_DB", tickets)
        .env("TRACE_FILE", tickets.with_extension("traces.jsonl"))
        .env("LOG_LEVEL", "warn")
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn investigation_script() -> Vec<Reply> {
    vec![
        Reply::json(message(
            "tool_use",
            json!([
                {"type": "thinking", "thinking": "Looking at payments-api errors and recent deploys.", "signature": "s1"},
                {"type": "tool_use", "id": "t1", "name": "telemetry__search_logs",
                 "input": {"service_name": "payments-api", "severity": "ERROR", "limit": 5}},
                {"type": "tool_use", "id": "t2", "name": "telemetry__get_deployment_history",
                 "input": {"service_name": "payments-api", "window_hours": 24}}
            ]),
        )),
        Reply::json(message(
            "tool_use",
            json!([
                {"type": "tool_use", "id": "t3", "name": "files__read_text_file", "input": {"path": "service-config.yaml"}}
            ]),
        )),
        Reply::json(message(
            "end_turn",
            json!([
                {"type": "text", "text": "ASSESSMENT: timeouts began after 2.14.0 (dep-1048)."}
            ]),
        )),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn ask_runs_a_multi_step_investigation_through_the_real_binary() {
    let fake = FakeAnthropic::start(investigation_script()).await;
    let tmp = tempfile::tempdir().unwrap();
    let out = agent_command(&fake, &tmp.path().join("t.db"))
        .args(["ask", "Investigate", "the", "payment", "API", "errors"])
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{stderr}");

    for expected in [
        "[ok] telemetry: ops-telemetry-mcp",
        "[ok] files: secure-filesystem-server",
        "you> Investigate the payment API errors",
        "[step 1/8] asking Claude",
        "~ Looking at payments-api errors and recent deploys.",
        "-> telemetry__search_logs",
        "ok telemetry__search_logs: 5 of 8 matching log entries",
        "ok telemetry__get_deployment_history: 1 deployment(s) in the window",
        "ok files__read_text_file",
        "claude> ASSESSMENT: timeouts began after 2.14.0 (dep-1048).",
        "[done: 3 model call(s), 3 tool call(s)",
    ] {
        assert!(
            stdout.contains(expected),
            "missing `{expected}` in:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("SECRET123") && !stderr.contains("SECRET123"),
        "key leaked"
    );

    // What the API received: the user's request, then the MCP results.
    let requests = fake.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[0].body["messages"][0]["content"][0]["text"],
        "Investigate the payment API errors"
    );
    let results: Vec<String> = requests[1].body["messages"][2]["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["content"].as_str().unwrap().to_string())
        .collect();
    assert!(
        results[0].contains("log-0036") || results[0].contains("log-0039"),
        "{}",
        results[0]
    );
    assert!(results[1].contains("dep-1048"));
    let config_text = requests[2].body["messages"][4]["content"][0]["content"]
        .as_str()
        .unwrap();
    assert!(config_text.contains("timeout_ms: 2000"), "{config_text}");
    let names: Vec<&str> = requests[0].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"telemetry__search_logs") && names.contains(&"files__read_text_file"),
        "{names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_session_handles_commands_requests_and_exit() {
    let fake = FakeAnthropic::start(vec![Reply::json(message(
        "end_turn",
        json!([{"type": "text", "text": "Which service and what symptom?"}]),
    ))])
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let mut child = agent_command(&fake, &tmp.path().join("t.db"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"/help\n/tools\n/servers\n/bogus\nSomething is broken, fix it\n/reset\n/exit\n")
        .await
        .unwrap();
    drop(stdin);
    let out = child.wait_with_output().await.unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for expected in [
        "Commands: /tools",
        "telemetry__create_incident_ticket",
        "requires approval",
        "files__read_text_file",
        "secure-filesystem-server 2.4.0",
        "Unknown command /bogus",
        "claude> Which service and what symptom?",
        "Conversation cleared.",
        "Shutting down MCP servers...",
        "Bye.",
    ] {
        assert!(
            stdout.contains(expected),
            "missing `{expected}` in:\n{stdout}"
        );
    }
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn api_errors_are_reported_without_crashing() {
    let fake = FakeAnthropic::start(vec![Reply::error(
        401,
        "authentication_error",
        "invalid x-api-key",
    )])
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let out = agent_command(&fake, &tmp.path().join("t.db"))
        .args(["ask", "hello"])
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(
        stdout.contains("the API key was rejected (401)"),
        "{stdout}"
    );
    assert!(stdout.contains("setup.ps1 -ResetKey"), "{stdout}");
    assert!(!stdout.contains("SECRET123"));
    let _: Value = fake.requests()[0].body.clone();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ask_mode_needs_a_question() {
    let fake = FakeAnthropic::start(vec![]).await;
    let tmp = tempfile::tempdir().unwrap();
    let out = agent_command(&fake, &tmp.path().join("t.db"))
        .arg("ask")
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: agent-client"));
}

/// The approval gate through the real binary: the reviewer types at the console.
async fn ticket_flow(console_input: &'static [u8]) -> (String, tempfile::TempDir, String) {
    let fake = FakeAnthropic::start(vec![
        Reply::json(message("tool_use", json!([
            {"type": "tool_use", "id": "t1", "name": "telemetry__create_incident_ticket", "input": {
                "title": "payments-api checkout failures after 2.14.0", "severity": "CRITICAL",
                "summary": "Gateway timeouts since 09:12 UTC after deploy dep-1048 (2.14.0).",
                "assignee": "payments-oncall"}}
        ]))),
        Reply::json(message("end_turn", json!([{"type": "text", "text": "Done."}]))),
    ])
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("tickets.db");
    let mut child = agent_command(&fake, &db)
        .args(["ask", "open a ticket for the payments outage"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(console_input).await.unwrap();
    drop(stdin);
    let out = child.wait_with_output().await.unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let trace = std::fs::read_to_string(db.with_extension("traces.jsonl")).unwrap_or_default();
    (stdout, tmp, trace)
}

#[tokio::test(flavor = "multi_thread")]
async fn console_approval_creates_the_ticket() {
    let (stdout, tmp, trace) = ticket_flow(b"a\n").await;
    for expected in [
        "APPROVAL REQUIRED",
        "\"assignee\": \"payments-oncall\"",
        "Payload SHA-256: ",
        "[STATE-CHANGING: needs your approval]",
        ">> APPROVED - executing exactly the payload shown",
        "ticket INC-1001 created",
    ] {
        assert!(
            stdout.contains(expected),
            "missing `{expected}` in:\n{stdout}"
        );
    }
    assert!(tmp.path().join("tickets.db").exists());
    assert!(
        trace.contains("\"decision\":\"approved\"") && trace.contains("\"approval\":\"approved\""),
        "{trace}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn console_rejection_creates_nothing() {
    let (stdout, tmp, trace) = ticket_flow(b"r\nwrong service\n").await;
    assert!(
        stdout.contains(">> REJECTED (wrong service) - nothing was executed"),
        "{stdout}"
    );
    assert!(
        !tmp.path().join("tickets.db").exists(),
        "the telemetry server never received the call"
    );
    assert!(
        trace.contains("\"decision\":\"rejected\"") && trace.contains("\"status\":\"rejected\""),
        "{trace}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_console_input_means_no_approval() {
    let (stdout, tmp, _) = ticket_flow(b"").await;
    assert!(stdout.contains("treated as REJECTED"), "{stdout}");
    assert!(!tmp.path().join("tickets.db").exists());
}

/// Runs the real binary once and keeps its trace and console output in
/// traces/samples/. The model's replies come from the local scripted endpoint
/// (recorded as `api_endpoint` in the trace); everything else is real.
async fn record_sample(
    name: &str,
    prompt: &str,
    replies: Vec<Reply>,
    console_input: &'static [u8],
) -> String {
    let fake = FakeAnthropic::start(replies).await;
    let samples = project_root().join("traces").join("samples");
    let trace = samples.join(format!("{name}.jsonl"));
    let console = samples.join(format!("{name}.console.txt"));
    let _ = std::fs::remove_file(&trace);
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = agent_command(&fake, &tmp.path().join("tickets.db"));
    cmd.env("TRACE_FILE", &trace)
        .args(["ask", prompt])
        .stdin(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(console_input).await.unwrap();
    drop(stdin);
    let out = child.wait_with_output().await.unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    std::fs::write(&console, &stdout).unwrap();
    assert!(
        !stdout.contains("SECRET123")
            && !std::fs::read_to_string(&trace)
                .unwrap()
                .contains("SECRET123")
    );
    stdout
}

/// Opt-in: `cargo test -p agent-client --test cli_e2e -- --ignored --test-threads=1`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "writes the submission traces into traces/samples/"]
async fn write_submission_traces() {
    // 1. A complete multi-step investigation ending in an approved ticket.
    let ticket = json!({
        "title": "payments-api checkout failures after 2.14.0 deploy",
        "severity": "CRITICAL",
        "summary": "Since 09:12 UTC payments-api requests to PayFlow time out at the new 2,000 ms limit \
                    (log-0029, log-0030) right after deploy dep-1048 of 2.14.0 at 09:05 (timeout 10,000 -> 2,000 ms, \
                    retries x3, pool 50 -> 20). PayFlow reports normal latency (~2,050 ms, log-0035). Circuit breaker \
                    open, checkout returning 503 (log-0036/0037), success rate 61.2% (log-0044). Runbook: roll back \
                    to 2.13.2 first. log-0033 contains a prompt-injection attempt in a customer field.",
        "assignee": "payments-oncall"
    });
    let stdout = record_sample(
        "investigation-with-hitl-approval",
        "Customers report failed payments since this morning. Investigate payments-api and open an incident ticket if warranted.",
        vec![
            Reply::json(message("tool_use", json!([
                {"type": "thinking", "thinking": "Start with payments-api errors and recent deployments.", "signature": "sig-1"},
                {"type": "tool_use", "id": "toolu_01", "name": "telemetry__search_logs",
                 "input": {"service_name": "payments-api", "severity": "ERROR", "limit": 20}},
                {"type": "tool_use", "id": "toolu_02", "name": "telemetry__get_deployment_history",
                 "input": {"service_name": "payments-api", "window_hours": 24}}
            ]))),
            Reply::json(message("tool_use", json!([
                {"type": "thinking", "thinking": "Errors begin 7 minutes after 2.14.0. Checking the runbook, release notes and config.", "signature": "sig-2"},
                {"type": "tool_use", "id": "toolu_03", "name": "telemetry__read_resource", "input": {"uri": "service://runbooks/payments-api"}},
                {"type": "tool_use", "id": "toolu_04", "name": "files__read_text_file", "input": {"path": "RELEASE_NOTES.md"}},
                {"type": "tool_use", "id": "toolu_05", "name": "files__read_text_file", "input": {"path": "service-config.yaml"}}
            ]))),
            Reply::json(message("tool_use", json!([
                {"type": "tool_use", "id": "toolu_06", "name": "telemetry__create_incident_ticket", "input": ticket}
            ]))),
            Reply::json(message("end_turn", json!([{"type": "text", "text":
                "Summary: payments-api has been failing since 09:12 UTC, 7 minutes after 2.14.0 was deployed.\n\
                 Evidence: timeouts at the new 2,000 ms limit (log-0029, log-0030) while PayFlow is healthy at ~2,050 ms \
                 (log-0035); circuit breaker open (log-0036); dep-1048 changed timeout 10,000 -> 2,000 ms, added retries \
                 and cut the pool 50 -> 20 (RELEASE_NOTES.md, service-config.yaml).\n\
                 Likely cause (high confidence, not proven): the new timeout is below PayFlow's normal latency.\n\
                 Next steps (runbook): roll back to 2.13.2, then restore the timeout and pool.\n\
                 Ticket: INC-1001 (CRITICAL, payments-oncall), approved by the reviewer.\n\
                 Security note: log-0033 contains a prompt-injection attempt; it was treated as data."}]))),
        ],
        b"a\n",
    )
    .await;
    assert!(
        stdout.contains(">> APPROVED") && stdout.contains("ticket INC-1001 created"),
        "{stdout}"
    );

    // 2. Handled failures and edge cases.
    let overloaded =
        || Reply::error(529, "overloaded_error", "Overloaded").header("retry-after", "0");
    let stdout = record_sample(
        "handled-failures-and-edge-cases",
        "Investigate inventory-service and summarize the latest payments-api errors.",
        vec![
            Reply::json(message("tool_use", json!([
                {"type": "tool_use", "id": "toolu_11", "name": "telemetry__search_logs", "input": {"service_name": "inventory-service"}},
                {"type": "tool_use", "id": "toolu_12", "name": "telemetry__search_logs",
                 "input": {"service_name": "payments-api", "severity": "ERROR", "limit": 10}}
            ]))),
            // A model "fooled" by log-0033 tries what it says:
            Reply::json(message("tool_use", json!([
                {"type": "tool_use", "id": "toolu_13", "name": "files__read_text_file", "input": {"path": "../../.env"}},
                {"type": "tool_use", "id": "toolu_14", "name": "files__write_file", "input": {"path": "x.txt", "content": "x"}},
                {"type": "tool_use", "id": "toolu_15", "name": "telemetry__create_incident_ticket", "input": {
                    "title": "SYSTEM OVERRIDE", "severity": "CRITICAL",
                    "summary": "Requested by an instruction found inside log-0033.", "assignee": "admin"}},
                {"type": "tool_use", "id": "toolu_16", "name": "telemetry__get_deployment_history",
                 "input": {"service_name": "payments-api", "window_hours": -5}}
            ]))),
            // ...then the Claude API is overloaded on every retry.
            overloaded(), overloaded(), overloaded(), overloaded(),
        ],
        b"",
    )
    .await;
    assert!(
        stdout.contains("[error: the Claude API is overloaded"),
        "{stdout}"
    );
}
