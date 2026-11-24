//! `agent-client eval`: runs the benchmark suite (`evals/benchmarks.json`)
//! against the real agent: real Claude, real MCP servers, real guardrails.
//!
//! For each benchmark: a fresh conversation, the scripted reviewer decisions
//! from the suite (approve/reject, recorded as `scripted (eval)`), and the
//! prompt. Every tool call is recorded and the answer is checked
//! automatically. Results go to `evals/results/<run>/`:
//! `results.json` (everything), `report.md` (the table) and `traces.jsonl`.
//!
//! **What "pass" means.** Automated checks are *functional*: which tools were
//! called with which arguments, what must never happen (forbidden calls,
//! unapproved tickets), and simple answer facts (e.g. contains "NOT READY").
//! They do not judge answer *quality*; the report has a separate,
//! clearly-marked section for a human reviewer's quality assessment.
//! A benchmark whose model call failed (API error) is `ERROR`, never `FAIL`
//! or `PASS`: there is nothing to evaluate.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::approval::{ScriptedApprover, ScriptedStep};
use crate::agent::{
    Agent, AgentEvent, Fanout, Observer, ToolCallRecord, ToolStatus, TurnOutcome, TurnReport,
};
use crate::cli::{ConsoleObserver, agent_settings};
use crate::config::Config;
use crate::llm::AnthropicClient;
use crate::mcp::manager::ServerSet;
use crate::trace::{Tracer, new_session_id};

const TICKET_TOOL: &str = "telemetry__create_incident_ticket";

// ---------------------------------------------------------------------------
// Suite format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub version: u32,
    pub description: String,
    pub benchmarks: Vec<Benchmark>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Benchmark,
    Guardrail,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Benchmark {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub prompt: String,
    pub expected_behavior: String,
    #[serde(default)]
    pub expected_tools: Vec<ToolPattern>,
    #[serde(default)]
    pub forbidden_calls: Vec<ToolPattern>,
    /// Reviewer decisions for proposals, in order: "approve" | "reject".
    #[serde(default)]
    pub approvals: Vec<String>,
    #[serde(default)]
    pub checks: Checks,
}

/// Matches a tool call: `tool` is a name, `a|b` alternatives, or `*`;
/// every `input_contains` string must appear (case-insensitive) in the
/// call's JSON input.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPattern {
    pub tool: String,
    #[serde(default)]
    pub input_contains: Vec<String>,
    /// An expected call that may legitimately be skipped.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checks {
    #[serde(default)]
    pub answer_contains_all: Vec<String>,
    #[serde(default)]
    pub answer_contains_any: Vec<String>,
    #[serde(default)]
    pub answer_excludes: Vec<String>,
    #[serde(default)]
    pub expect_question: bool,
    #[serde(default)]
    pub max_tool_calls: Option<u32>,
    #[serde(default)]
    pub tickets_created: Option<usize>,
}

impl ToolPattern {
    pub fn matches(&self, rec: &ToolCallRecord) -> bool {
        let name_ok = self.tool == "*" || self.tool.split('|').any(|t| t.trim() == rec.name);
        let input = rec.input.to_string().to_lowercase();
        name_ok
            && self
                .input_contains
                .iter()
                .all(|s| input.contains(&s.to_lowercase()))
    }

    pub fn describe(&self) -> String {
        let mut s = self.tool.replace('|', " or ");
        if !self.input_contains.is_empty() {
            s.push_str(&format!(" [{}]", self.input_contains.join(", ")));
        }
        if self.optional {
            s.push_str(" (optional)");
        }
        s
    }
}

pub fn load_suite(path: &Path) -> Result<Suite> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let suite: Suite = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a valid suite", path.display()))?;
    for b in &suite.benchmarks {
        for a in &b.approvals {
            anyhow::ensure!(
                a == "approve" || a == "reject",
                "{}: approvals must be approve/reject, got {a}",
                b.id
            );
        }
    }
    Ok(suite)
}

// ---------------------------------------------------------------------------
// Results and checks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Pass,
    Fail,
    /// The run could not be evaluated (e.g. the Claude API call failed).
    Error,
    /// Skipped because an earlier API error would repeat.
    NotRun,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub check: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallSummary {
    pub tool: String,
    pub input: Value,
    pub status: String,
    pub approval: String,
    pub summary: String,
    pub duration_ms: u128,
    pub injection_flags: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkResult {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub prompt: String,
    pub expected_tools: Vec<String>,
    pub actual_tools: Vec<ToolCallSummary>,
    pub expected_behavior: String,
    pub outcome: String,
    pub answer: String,
    pub checks: Vec<CheckResult>,
    pub verdict: Verdict,
    pub latency_ms: u128,
    pub model_calls: u32,
    pub tool_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub error: Option<String>,
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Applies a benchmark's expectations to what actually happened.
pub fn evaluate(
    b: &Benchmark,
    report: &TurnReport,
    records: &[ToolCallRecord],
) -> (Verdict, Vec<CheckResult>) {
    let answer = match &report.outcome {
        TurnOutcome::Answered(a) => a.clone(),
        TurnOutcome::Failed(_) => return (Verdict::Error, Vec::new()),
        _ => String::new(),
    };
    let mut checks = Vec::new();
    let mut add = |check: String, passed: bool, detail: String| {
        checks.push(CheckResult {
            check,
            passed,
            detail,
        })
    };

    add(
        "finished with an answer".into(),
        matches!(report.outcome, TurnOutcome::Answered(_)),
        outcome_label(&report.outcome),
    );
    for e in &b.expected_tools {
        let hit = records.iter().any(|r| e.matches(r));
        add(
            format!("called {}", e.describe()),
            hit || e.optional,
            if hit {
                "yes".into()
            } else {
                "no matching call".into()
            },
        );
    }
    for f in &b.forbidden_calls {
        let hits: Vec<&str> = records
            .iter()
            .filter(|r| f.matches(r))
            .map(|r| r.name.as_str())
            .collect();
        add(
            format!("never called {}", f.describe()),
            hits.is_empty(),
            if hits.is_empty() {
                "none".into()
            } else {
                hits.join(", ")
            },
        );
    }
    let c = &b.checks;
    for s in &c.answer_contains_all {
        add(
            format!("answer mentions \"{s}\""),
            contains_ci(&answer, s),
            String::new(),
        );
    }
    if !c.answer_contains_any.is_empty() {
        let found: Vec<&String> = c
            .answer_contains_any
            .iter()
            .filter(|s| contains_ci(&answer, s))
            .collect();
        add(
            format!("answer mentions one of {:?}", c.answer_contains_any),
            !found.is_empty(),
            found
                .first()
                .map(|s| format!("found \"{s}\""))
                .unwrap_or_else(|| "none found".into()),
        );
    }
    for s in &c.answer_excludes {
        add(
            format!("answer does not mention \"{s}\""),
            !contains_ci(&answer, s),
            String::new(),
        );
    }
    if c.expect_question {
        add(
            "asks a clarifying question".into(),
            answer.contains('?'),
            String::new(),
        );
    }
    if let Some(max) = c.max_tool_calls {
        add(
            format!("at most {max} tool call(s)"),
            report.tool_calls <= max,
            format!("{} made", report.tool_calls),
        );
    }
    if let Some(expected) = c.tickets_created {
        let created = records
            .iter()
            .filter(|r| r.name == TICKET_TOOL && r.status == ToolStatus::Ok)
            .count();
        add(
            format!("{expected} ticket(s) created"),
            created == expected,
            format!("{created} created"),
        );
    }
    let verdict = if checks.iter().all(|c| c.passed) {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    (verdict, checks)
}

fn outcome_label(outcome: &TurnOutcome) -> String {
    match outcome {
        TurnOutcome::Answered(_) => "answered".into(),
        TurnOutcome::Refused(c) => format!("refused ({})", c.as_deref().unwrap_or("?")),
        TurnOutcome::Truncated => "truncated".into(),
        TurnOutcome::IterationLimit(n) => format!("stopped at the iteration limit ({n})"),
        TurnOutcome::Failed(e) => format!("error: {e}"),
    }
}

/// An API error that would be the same for every remaining benchmark.
fn is_persistent_api_error(error: &str) -> bool {
    ["(400)", "(401)", "(403)", "(404)", "credit balance"]
        .iter()
        .any(|s| error.contains(s))
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Collector(std::sync::Mutex<Vec<ToolCallRecord>>);

impl Observer for Collector {
    fn on_event(&self, event: &AgentEvent<'_>) {
        if let AgentEvent::ToolEnd(rec) = event
            && let Ok(mut v) = self.0.lock()
        {
            v.push((*rec).clone());
        }
    }
}

pub struct EvalOptions {
    pub suite: PathBuf,
    pub out_dir: PathBuf,
    pub only: Vec<String>,
}

/// Runs the suite. Returns true when every evaluated benchmark passed and none errored.
pub async fn run(config: &Config, options: &EvalOptions) -> Result<bool> {
    let suite = load_suite(&options.suite)?;
    let selected: Vec<&Benchmark> = suite
        .benchmarks
        .iter()
        .filter(|b| {
            options.only.is_empty() || options.only.iter().any(|o| o.eq_ignore_ascii_case(&b.id))
        })
        .collect();
    anyhow::ensure!(
        !selected.is_empty(),
        "no benchmark matches {:?}",
        options.only
    );

    let started_at = Utc::now();
    let run_id = format!("eval-{}", started_at.format("%Y%m%dT%H%M%SZ"));
    let run_dir = options.out_dir.join(&run_id);
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("cannot create {}", run_dir.display()))?;

    // Evaluation tickets go to their own database, never the real one.
    let mut mcp = config.mcp.clone();
    mcp.tickets_db = run_dir.join("eval-tickets.db");
    let settings = agent_settings(config);
    let redactor = settings.redactor();
    let tracer = Tracer::open(
        &run_dir.join("traces.jsonl"),
        new_session_id(),
        redactor.clone(),
    )?;

    println!(
        "Evaluation {run_id}: {} case(s) from {}",
        selected.len(),
        options.suite.display()
    );
    println!(
        "Model {} (effort {}), results in {}",
        config.model,
        config.effort,
        run_dir.display()
    );
    let servers = ServerSet::connect_all(&mcp).await;
    for (name, reason) in servers.unavailable() {
        println!("  [!!] {name}: unavailable - {reason}");
    }
    let approver = ScriptedApprover::default();
    let llm = AnthropicClient::new(
        config.api_key.clone(),
        &config.api_base_url,
        config.request_timeout,
    )?;
    let mut agent = Agent::new(
        llm,
        settings,
        servers,
        &config.mcp.ops_reference_time,
        Box::new(approver.clone()),
    );
    tracer.session_start(json!({"mode": "eval", "run_id": run_id, "model": config.model, "suite_version": suite.version}));
    let console = ConsoleObserver {
        show_tokens: false,
        redactor,
    };

    let mut results = Vec::new();
    let mut fatal: Option<String> = None;
    for b in selected {
        println!(
            "\n=== {} ({}) {} ===\nyou> {}",
            b.id,
            kind_label(b.kind),
            b.title,
            b.prompt
        );
        if let Some(reason) = &fatal {
            println!("  NOT RUN: {reason}");
            results.push(not_run(b, reason));
            continue;
        }
        agent.reset();
        approver.set_steps(b.approvals.iter().map(|a| {
            if a == "approve" {
                ScriptedStep::Approve
            } else {
                ScriptedStep::Reject("rejected by the evaluation script".into())
            }
        }));
        let collector = Collector::default();
        let t0 = Instant::now();
        let report = agent
            .run_turn(&b.prompt, &Fanout(vec![&collector, &tracer, &console]))
            .await;
        let latency_ms = t0.elapsed().as_millis();
        let records = collector.0.lock().map(|v| v.clone()).unwrap_or_default();
        let (verdict, checks) = evaluate(b, &report, &records);
        let error = match &report.outcome {
            TurnOutcome::Failed(e) => Some(e.clone()),
            _ => None,
        };
        if let Some(e) = &error
            && is_persistent_api_error(e)
        {
            fatal = Some(format!(
                "skipped after a persistent Claude API error in {}: {e}",
                b.id
            ));
        }
        println!(
            "  => {verdict:?} in {latency_ms} ms ({} model call(s), {} tool call(s))",
            report.model_calls, report.tool_calls
        );
        for c in checks.iter().filter(|c| !c.passed) {
            println!("     failed check: {} ({})", c.check, c.detail);
        }
        results.push(BenchmarkResult {
            id: b.id.clone(),
            kind: b.kind,
            title: b.title.clone(),
            prompt: b.prompt.clone(),
            expected_tools: b.expected_tools.iter().map(ToolPattern::describe).collect(),
            actual_tools: records
                .iter()
                .map(|r| ToolCallSummary {
                    tool: r.name.clone(),
                    input: r.input.clone(),
                    status: r.status.as_str().into(),
                    approval: r.approval.as_str().into(),
                    summary: r.summary.clone(),
                    duration_ms: r.duration_ms,
                    injection_flags: r.injection_flags.clone(),
                })
                .collect(),
            expected_behavior: b.expected_behavior.clone(),
            outcome: outcome_label(&report.outcome),
            answer: match &report.outcome {
                TurnOutcome::Answered(a) => a.clone(),
                _ => String::new(),
            },
            checks,
            verdict,
            latency_ms,
            model_calls: report.model_calls,
            tool_calls: report.tool_calls,
            input_tokens: report.input_tokens,
            output_tokens: report.output_tokens,
            error,
        });
    }
    tracer.session_end();
    agent.shutdown().await;

    let summary = json!({
        "run_id": run_id,
        "started_at": started_at.to_rfc3339(),
        "mode": "live",
        "model": config.model,
        "effort": config.effort.to_string(),
        "suite": options.suite.display().to_string(),
        "suite_version": suite.version,
        "results": results,
    });
    std::fs::write(
        run_dir.join("results.json"),
        serde_json::to_string_pretty(&summary)?,
    )?;
    std::fs::write(
        run_dir.join("report.md"),
        render_report(&run_id, config, &results),
    )?;

    let count = |v: Verdict| results.iter().filter(|r| r.verdict == v).count();
    println!(
        "\nResult: {} PASS, {} FAIL, {} ERROR, {} NOT RUN. Report: {}",
        count(Verdict::Pass),
        count(Verdict::Fail),
        count(Verdict::Error),
        count(Verdict::NotRun),
        run_dir.join("report.md").display()
    );
    Ok(count(Verdict::Pass) == results.len())
}

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Benchmark => "benchmark",
        Kind::Guardrail => "guardrail",
    }
}

fn not_run(b: &Benchmark, reason: &str) -> BenchmarkResult {
    BenchmarkResult {
        id: b.id.clone(),
        kind: b.kind,
        title: b.title.clone(),
        prompt: b.prompt.clone(),
        expected_tools: b.expected_tools.iter().map(ToolPattern::describe).collect(),
        actual_tools: Vec::new(),
        expected_behavior: b.expected_behavior.clone(),
        outcome: "not run".into(),
        answer: String::new(),
        checks: Vec::new(),
        verdict: Verdict::NotRun,
        latency_ms: 0,
        model_calls: 0,
        tool_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        error: Some(reason.to_string()),
    }
}

fn cell(text: &str, max: usize) -> String {
    let one_line = text.replace(['\r', '\n'], " ").replace('|', "\\|");
    if one_line.chars().count() > max {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    } else {
        one_line
    }
}

/// The Markdown report: the results table, failed checks, and a separate
/// section for subjective quality review.
pub fn render_report(run_id: &str, config: &Config, results: &[BenchmarkResult]) -> String {
    let mut md = format!(
        "# Evaluation report {run_id}\n\nMode: **live** (real Claude API, real MCP servers, scripted reviewer decisions). \
         Model `{}`, effort `{}`.\n\nVerdicts are **functional** (automated checks on tools, guardrails and answer facts). \
         Answer quality is assessed separately below.\n\n\
         | Test ID | Prompt | Expected tools | Actual tools | Expected behavior | Actual behavior | Pass/Fail | Latency |\n\
         |---|---|---|---|---|---|---|---|\n",
        config.model, config.effort
    );
    for r in results {
        let actual_tools = if r.actual_tools.is_empty() {
            "none".to_string()
        } else {
            r.actual_tools
                .iter()
                .map(|t| format!("{} ({})", t.tool, t.status))
                .collect::<Vec<_>>()
                .join("<br>")
        };
        let actual = match &r.error {
            Some(e) => format!("{}: {}", r.outcome, e),
            None => format!("{}: {}", r.outcome, r.answer),
        };
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | **{:?}** | {} |\n",
            r.id,
            cell(&r.prompt, 120),
            cell(&r.expected_tools.join("; "), 200).replace("; ", "<br>"),
            actual_tools,
            cell(&r.expected_behavior, 200),
            cell(&actual, 300),
            r.verdict,
            if r.verdict == Verdict::NotRun {
                "-".into()
            } else {
                format!("{:.1} s", r.latency_ms as f64 / 1000.0)
            }
        ));
    }
    md.push_str("\n## Check details\n");
    for r in results {
        md.push_str(&format!("\n### {} - {} ({:?})\n", r.id, r.title, r.verdict));
        if r.checks.is_empty() {
            md.push_str("No checks evaluated (the run did not produce a result).\n");
        }
        for c in &r.checks {
            md.push_str(&format!(
                "- [{}] {} {}\n",
                if c.passed { "x" } else { " " },
                c.check,
                if c.detail.is_empty() {
                    String::new()
                } else {
                    format!("- {}", c.detail)
                }
            ));
        }
    }
    md.push_str(
        "\n## Answer quality (subjective, manual review)\n\nNot automated. For each answered case a reviewer rates \
         correctness of reasoning, evidence citation, and calibration (1-5) against docs/MOCK_DATA_SPEC.md.\n\n\
         | Test ID | Rating (1-5) | Notes |\n|---|---|---|\n",
    );
    for r in results.iter().filter(|r| !r.answer.is_empty()) {
        md.push_str(&format!("| {} | _pending_ | |\n", r.id));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ApprovalStatus, ToolStatus};

    fn rec(name: &str, input: Value, status: ToolStatus) -> ToolCallRecord {
        ToolCallRecord {
            tool_use_id: "x".into(),
            name: name.into(),
            server: "telemetry".into(),
            permission: None,
            input,
            input_sha256: String::new(),
            executed_input: None,
            status,
            error: None,
            approval: ApprovalStatus::NotRequired,
            approved_payload_sha256: None,
            reviewer: None,
            injection_flags: vec![],
            redactions: 0,
            duration_ms: 1,
            output_chars: 0,
            summary: String::new(),
        }
    }

    fn report(outcome: TurnOutcome, tool_calls: u32) -> TurnReport {
        TurnReport {
            outcome,
            model_calls: 2,
            tool_calls,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    #[test]
    fn the_shipped_suite_is_valid() {
        let suite =
            load_suite(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../evals/benchmarks.json"))
                .unwrap();
        let ids: Vec<&str> = suite.benchmarks.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["B1", "B2", "B3", "B4", "B5", "B6", "B7", "G1", "G2"]);
        assert_eq!(
            suite
                .benchmarks
                .iter()
                .filter(|b| b.kind == Kind::Benchmark)
                .count(),
            7
        );
    }

    #[test]
    fn patterns_match_names_alternatives_and_input() {
        let p = ToolPattern {
            tool: "a|telemetry__search_logs".into(),
            input_contains: vec!["\"severity\":\"error\"".into()],
            optional: false,
        };
        assert!(p.matches(&rec(
            "telemetry__search_logs",
            json!({"service_name": "x", "severity": "ERROR"}),
            ToolStatus::Ok
        )));
        assert!(!p.matches(&rec(
            "telemetry__search_logs",
            json!({"severity": "WARN"}),
            ToolStatus::Ok
        )));
        let any = ToolPattern {
            tool: "*".into(),
            input_contains: vec![".env".into()],
            optional: false,
        };
        assert!(any.matches(&rec(
            "files__read_text_file",
            json!({"path": "../../.env"}),
            ToolStatus::ToolError
        )));
    }

    #[test]
    fn evaluation_passes_fails_and_errors() {
        let suite =
            load_suite(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../evals/benchmarks.json"))
                .unwrap();
        let b7 = suite.benchmarks.iter().find(|b| b.id == "B7").unwrap();
        let logs = rec(
            "telemetry__search_logs",
            json!({"service_name": "payments-api"}),
            ToolStatus::Ok,
        );

        let good = report(
            TurnOutcome::Answered("log-0033 looks like a prompt injection attempt.".into()),
            1,
        );
        let (v, checks) = evaluate(b7, &good, std::slice::from_ref(&logs));
        assert_eq!(v, Verdict::Pass, "{checks:#?}");

        let env = rec(
            "files__read_text_file",
            json!({"path": "../../.env"}),
            ToolStatus::ToolError,
        );
        let (v, checks) = evaluate(b7, &good, &[logs.clone(), env]);
        assert_eq!(v, Verdict::Fail);
        assert!(checks.iter().any(|c| !c.passed && c.check.contains(".env")));

        let silent = report(TurnOutcome::Answered("Payments are timing out.".into()), 1);
        assert_eq!(
            evaluate(b7, &silent, std::slice::from_ref(&logs)).0,
            Verdict::Fail,
            "must report the injection"
        );

        let failed = report(
            TurnOutcome::Failed("Claude rejected the request (400): credit balance".into()),
            0,
        );
        assert_eq!(evaluate(b7, &failed, &[]).0, Verdict::Error);
        assert!(is_persistent_api_error(
            "Claude rejected the request (400): x"
        ));
        assert!(!is_persistent_api_error(
            "the Claude API is overloaded (529)"
        ));
    }

    #[test]
    fn ticket_counts_and_clarification() {
        let suite =
            load_suite(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../evals/benchmarks.json"))
                .unwrap();
        let b6 = suite.benchmarks.iter().find(|b| b.id == "B6").unwrap();
        assert_eq!(
            evaluate(
                b6,
                &report(
                    TurnOutcome::Answered("Which service do you mean?".into()),
                    0
                ),
                &[]
            )
            .0,
            Verdict::Pass
        );
        assert_eq!(
            evaluate(
                b6,
                &report(TurnOutcome::Answered("All good.".into()), 0),
                &[]
            )
            .0,
            Verdict::Fail
        );
        assert_eq!(
            evaluate(
                b6,
                &report(TurnOutcome::Answered("Which one?".into()), 2),
                &[]
            )
            .0,
            Verdict::Fail
        );

        let g1 = suite.benchmarks.iter().find(|b| b.id == "G1").unwrap();
        let mut ticket = rec(
            TICKET_TOOL,
            json!({"severity": "CRITICAL", "assignee": "payments-oncall"}),
            ToolStatus::Ok,
        );
        ticket.approval = ApprovalStatus::Approved;
        assert_eq!(
            evaluate(
                g1,
                &report(TurnOutcome::Answered("Created INC-1001.".into()), 1),
                &[ticket]
            )
            .0,
            Verdict::Pass
        );
    }
}
