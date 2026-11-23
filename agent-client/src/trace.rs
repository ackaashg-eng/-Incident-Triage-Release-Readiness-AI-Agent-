//! Append-only JSONL execution traces (`traces/agent_traces.jsonl`).
//!
//! One JSON object per line. Every line has `ts` (UTC, ms), `session_id`,
//! `turn`, `iteration` and `event`:
//!
//! | event | extra fields |
//! |---|---|
//! | `session_start` | model, effort, servers, tool counts, max_iterations, trace format version |
//! | `user_request` | text |
//! | `model_call` | status (ok/error), stop_reason, tokens, duration_ms, error |
//! | `approval` | tool, server, proposed/approved payload hashes, decision, edited, reviewer, reason |
//! | `tool_call` | tool, server, permission, input, input_sha256, status, error, approval, duration_ms, injection_flags, redactions, output_chars |
//! | `warning` | message |
//! | `turn_end` | outcome, model_calls, tool_calls, tokens, answer (shortened) |
//! | `session_end` | — |
//!
//! Every line passes through the [`Redactor`] before it is written (secret
//! patterns, sensitive JSON keys, and the API key's exact value). A failure to
//! write is logged once and never stops the agent.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use crate::agent::approval::Decision;
use crate::agent::{AgentEvent, Observer, TurnOutcome};
use crate::llm::StopReason;
use crate::safety::redact::Redactor;

pub const TRACE_FORMAT: u32 = 1;

pub struct Tracer {
    path: PathBuf,
    file: Mutex<File>,
    session_id: String,
    redactor: Redactor,
    position: Mutex<(u32, u32)>, // (turn, iteration)
    warned: AtomicBool,
}

pub fn new_session_id() -> String {
    format!(
        "s-{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S"),
        std::process::id()
    )
}

impl Tracer {
    /// Opens (creates) the trace file for appending.
    pub fn open(path: &Path, session_id: String, redactor: Redactor) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            session_id,
            redactor,
            position: Mutex::new((0, 0)),
            warned: AtomicBool::new(false),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Writes one event line. `fields` must be a JSON object.
    pub fn record(&self, event: &str, fields: Value) {
        let (turn, iteration) = self.position.lock().map(|p| *p).unwrap_or((0, 0));
        let mut line: Map<String, Value> = match self.redactor.json(&fields) {
            Value::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("data".into(), other);
                m
            }
        };
        line.insert(
            "ts".into(),
            json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        );
        line.insert("session_id".into(), json!(self.session_id));
        line.insert("turn".into(), json!(turn));
        line.insert("iteration".into(), json!(iteration));
        line.insert("event".into(), json!(event));
        let mut text = Value::Object(line).to_string();
        text.push('\n');
        let result = self
            .file
            .lock()
            .map_err(|_| std::io::Error::other("trace lock poisoned"))
            .and_then(|mut f| {
                f.write_all(text.as_bytes())?;
                f.flush()
            });
        if let Err(err) = result
            && !self.warned.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(path = %self.path.display(), error = %err, "could not write the trace file");
        }
    }

    pub fn session_start(&self, details: Value) {
        let mut d = details;
        d["trace_format"] = json!(TRACE_FORMAT);
        self.record("session_start", d);
    }

    pub fn session_end(&self) {
        self.record("session_end", json!({}));
    }

    fn set_position(&self, turn: Option<u32>, iteration: Option<u32>) {
        if let Ok(mut p) = self.position.lock() {
            if let Some(t) = turn {
                *p = (t, 0);
            }
            if let Some(i) = iteration {
                p.1 = i;
            }
        }
    }
}

fn stop_name(stop: &StopReason) -> String {
    match stop {
        StopReason::EndTurn => "end_turn".into(),
        StopReason::ToolUse => "tool_use".into(),
        StopReason::MaxTokens => "max_tokens".into(),
        StopReason::Refusal(_) => "refusal".into(),
        StopReason::PauseTurn => "pause_turn".into(),
        StopReason::StopSequence => "stop_sequence".into(),
        StopReason::Other(o) => o.clone(),
        StopReason::Missing => "none".into(),
    }
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}…", text.chars().take(max).collect::<String>())
    } else {
        text.to_string()
    }
}

impl Observer for Tracer {
    fn on_event(&self, event: &AgentEvent<'_>) {
        match event {
            AgentEvent::TurnStart { turn, text } => {
                self.set_position(Some(*turn), None);
                self.record("user_request", json!({"text": text}));
            }
            AgentEvent::ModelCall { iteration, .. } => self.set_position(None, Some(*iteration)),
            AgentEvent::ModelReply {
                stop,
                input_tokens,
                output_tokens,
                cache_read,
                millis,
            } => {
                let category = match stop {
                    StopReason::Refusal(c) => c.clone(),
                    _ => None,
                };
                self.record(
                    "model_call",
                    json!({
                        "status": "ok", "stop_reason": stop_name(stop), "refusal_category": category,
                        "input_tokens": input_tokens, "output_tokens": output_tokens,
                        "cache_read_input_tokens": cache_read, "duration_ms": millis
                    }),
                );
            }
            AgentEvent::ModelError { error, millis } => {
                self.record(
                    "model_call",
                    json!({"status": "error", "error": error, "duration_ms": millis}),
                );
            }
            AgentEvent::Approval { proposal, decision } => {
                let (kind, approved_hash, edited, reviewer, reason) = match decision {
                    Decision::Approved {
                        payload_sha256,
                        edited,
                        reviewer,
                        ..
                    } => (
                        "approved",
                        Some(payload_sha256.as_str()),
                        *edited,
                        Some(reviewer.as_str()),
                        None,
                    ),
                    Decision::Rejected { reason, reviewer } => (
                        "rejected",
                        None,
                        false,
                        Some(reviewer.as_str()),
                        Some(reason.as_str()),
                    ),
                    Decision::Unavailable(r) => {
                        ("unavailable", None, false, None, Some(r.as_str()))
                    }
                };
                self.record(
                    "approval",
                    json!({
                        "tool": proposal.tool, "server": proposal.server,
                        "proposed_payload": proposal.arguments,
                        "proposed_payload_sha256": proposal.payload_sha256,
                        "decision": kind, "approved_payload_sha256": approved_hash,
                        "edited": edited, "reviewer": reviewer, "reason": reason
                    }),
                );
            }
            AgentEvent::ToolEnd(rec) => {
                self.record(
                    "tool_call",
                    json!({
                        "tool_use_id": rec.tool_use_id, "tool": rec.name, "server": rec.server,
                        "permission": rec.permission.map(|p| p.to_string()),
                        "input": rec.input, "input_sha256": rec.input_sha256,
                        "executed_input": rec.executed_input,
                        "status": rec.status.as_str(), "error": rec.error,
                        "approval": rec.approval.as_str(),
                        "approved_payload_sha256": rec.approved_payload_sha256,
                        "reviewer": rec.reviewer,
                        "duration_ms": rec.duration_ms,
                        "injection_flags": rec.injection_flags,
                        "redactions": rec.redactions,
                        "output_chars": rec.output_chars
                    }),
                );
            }
            AgentEvent::Warning(message) => self.record("warning", json!({"message": message})),
            AgentEvent::TurnEnd(report) => {
                let (outcome, detail) = match &report.outcome {
                    TurnOutcome::Answered(text) => ("answered", Some(shorten(text, 2000))),
                    TurnOutcome::Refused(c) => ("refused", c.clone()),
                    TurnOutcome::Truncated => ("truncated", None),
                    TurnOutcome::IterationLimit(max) => {
                        ("iteration_limit", Some(format!("max {max}")))
                    }
                    TurnOutcome::Failed(e) => ("failed", Some(e.clone())),
                };
                self.record(
                    "turn_end",
                    json!({
                        "outcome": outcome, "detail": detail,
                        "model_calls": report.model_calls, "tool_calls": report.tool_calls,
                        "input_tokens": report.input_tokens, "output_tokens": report.output_tokens
                    }),
                );
            }
            AgentEvent::Note(_)
            | AgentEvent::Text(_)
            | AgentEvent::Fallback(_)
            | AgentEvent::ToolStart { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_json_with_common_fields_and_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces").join("t.jsonl");
        let t = Tracer::open(
            &path,
            "s-test".into(),
            Redactor::new(["super-secret-api-key-value".to_string()]),
        )
        .unwrap();
        t.session_start(json!({"model": "claude-opus-5-5"}));
        t.on_event(&AgentEvent::TurnStart {
            turn: 1,
            text: "my key is super-secret-api-key-value",
        });
        t.on_event(&AgentEvent::ModelCall {
            iteration: 2,
            max: 8,
        });
        t.record(
            "custom",
            json!({"password": "hunter2hunter2", "note": "sk-ant-api03-AAAAAAAAAAAAAAAA"}),
        );
        t.session_end();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        for l in &lines {
            assert_eq!(l["session_id"], "s-test");
            assert!(l["ts"].as_str().unwrap().ends_with('Z'));
        }
        assert_eq!(lines[0]["event"], "session_start");
        assert_eq!(lines[0]["trace_format"], 1);
        assert_eq!(lines[1]["text"], "my key is [REDACTED]");
        assert_eq!(
            (lines[2]["turn"].as_u64(), lines[2]["iteration"].as_u64()),
            (Some(1), Some(2))
        );
        assert_eq!(lines[2]["password"], "[REDACTED]");
        assert_eq!(lines[2]["note"], "[REDACTED]");
        assert!(
            !text.contains("super-secret")
                && !text.contains("hunter2")
                && !text.contains("sk-ant-api03")
        );
    }

    #[test]
    fn appends_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        Tracer::open(&path, "a".into(), Redactor::default())
            .unwrap()
            .session_end();
        Tracer::open(&path, "b".into(), Redactor::default())
            .unwrap()
            .session_end();
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
    }
}
