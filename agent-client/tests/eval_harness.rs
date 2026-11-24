//! The evaluation harness itself (not the model): `agent-client eval` run
//! through the real binary, real MCP servers, and a local fake Claude API with
//! scripted replies. Proves that verdicts, the persistent-error stop, and the
//! result files work, so a live run's report can be trusted.

#![allow(clippy::unwrap_used)]

mod common;

use common::{FakeAnthropic, Reply, message, project_root};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn eval_produces_pass_fail_error_and_not_run_verdicts_with_reports() {
    let tmp = tempfile::tempdir().unwrap();
    let suite = tmp.path().join("suite.json");
    std::fs::write(
        &suite,
        json!({
            "version": 1,
            "description": "harness test",
            "benchmarks": [
                {"id": "X1", "kind": "benchmark", "title": "lookup", "prompt": "Show payments-api errors.",
                 "expected_behavior": "uses search_logs",
                 "expected_tools": [{"tool": "telemetry__search_logs", "input_contains": ["payments-api"]}],
                 "checks": {"answer_contains_any": ["8 "], "tickets_created": 0}},
                {"id": "X2", "kind": "benchmark", "title": "ambiguous", "prompt": "Is everything okay?",
                 "expected_behavior": "asks", "checks": {"expect_question": true, "max_tool_calls": 0}},
                {"id": "X3", "kind": "benchmark", "title": "api error", "prompt": "anything",
                 "expected_behavior": "n/a"},
                {"id": "X4", "kind": "guardrail", "title": "skipped", "prompt": "anything",
                 "expected_behavior": "n/a"}
            ]
        })
        .to_string(),
    )
    .unwrap();

    let fake = FakeAnthropic::start(vec![
        // X1: one tool call, then an answer → PASS
        Reply::json(message(
            "tool_use",
            json!([{"type": "tool_use", "id": "t1", "name": "telemetry__search_logs",
            "input": {"service_name": "payments-api", "severity": "ERROR"}}]),
        )),
        Reply::json(message(
            "end_turn",
            json!([{"type": "text", "text": "There are 8 ERROR entries."}]),
        )),
        // X2: answers without asking → FAIL (expects a question)
        Reply::json(message(
            "end_turn",
            json!([{"type": "text", "text": "Everything is fine."}]),
        )),
        // X3: billing error → ERROR, and X4 is then NOT RUN
        Reply::error(
            400,
            "invalid_request_error",
            "Your credit balance is too low to access the Anthropic API.",
        ),
    ])
    .await;

    let out_dir = tmp.path().join("results");
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-client"))
        .current_dir(project_root())
        .args(["eval", "--suite"])
        .arg(&suite)
        .arg("--out")
        .arg(&out_dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test-SECRET123")
        .env("ANTHROPIC_BASE_URL", &fake.url)
        .env("LOG_LEVEL", "error")
        .env_remove("RUST_LOG")
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "not everything passed, so the exit code is non-zero"
    );
    assert!(
        stdout.contains("Result: 1 PASS, 1 FAIL, 1 ERROR, 1 NOT RUN"),
        "{stdout}"
    );
    assert_eq!(fake.requests().len(), 4, "X4 never called the API");

    let run_dir = std::fs::read_dir(&out_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let results: Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("results.json")).unwrap())
            .unwrap();
    let verdicts: Vec<&str> = results["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["verdict"].as_str().unwrap())
        .collect();
    assert_eq!(verdicts, ["PASS", "FAIL", "ERROR", "NOT_RUN"]);
    let x1 = &results["results"][0];
    assert_eq!(x1["actual_tools"][0]["tool"], "telemetry__search_logs");
    assert_eq!(x1["actual_tools"][0]["status"], "ok");
    assert!(x1["latency_ms"].as_u64().is_some());
    assert!(
        results["results"][2]["error"]
            .as_str()
            .unwrap()
            .contains("credit balance")
    );

    let report = std::fs::read_to_string(run_dir.join("report.md")).unwrap();
    assert!(report.contains("| Test ID | Prompt | Expected tools | Actual tools | Expected behavior | Actual behavior | Pass/Fail | Latency |"));
    assert!(
        report.contains("| X1 |") && report.contains("**Pass**") && report.contains("**NotRun**")
    );
    assert!(report.contains("Answer quality (subjective, manual review)"));
    assert!(
        report.contains("[ ] asks a clarifying question"),
        "the failed check is listed"
    );

    let traces = std::fs::read_to_string(run_dir.join("traces.jsonl")).unwrap();
    assert!(traces.contains("\"mode\":\"eval\"") && traces.contains("\"event\":\"tool_call\""));
    assert!(!traces.contains("SECRET123") && !stdout.contains("SECRET123"));
}
