//! Human-in-the-loop approval for state-changing tools.
//!
//! When Claude calls a tool the application policy classifies as
//! `RequiresApproval`, the agent **pauses**, validates the payload, and asks an
//! [`Approver`]. Nothing runs unless the approver returns
//! [`Decision::Approved`], and approval is **bound to the exact payload**: the
//! decision carries the SHA-256 of `server + tool + canonical JSON
//! arguments`, and the agent recomputes it over what it is about to execute. Any
//! difference (an edit that was not re-approved, a bug, tampering) blocks
//! execution.
//!
//! Approvers:
//! * [`ConsoleApprover`]: the real one. Shows the exact payload and its hash and
//!   reads `a` (approve), `r` (reject) or `e` (edit) **from the console**. Text
//!   from Claude or from tool output can never answer it. An edit shows the new
//!   payload and requires a fresh approval. End of input = reject.
//! * [`ScriptedApprover`]: pre-programmed decisions for tests and evaluations.
//! * [`DeferredApprover`]: approves nothing (non-interactive use).

use std::collections::VecDeque;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};

use super::validate::validate;

/// A state-changing action Claude asked for, exactly as it would be executed.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// Namespaced tool name, e.g. `telemetry__create_incident_ticket`.
    pub tool: String,
    pub server: &'static str,
    /// Arguments, already validated against `schema`.
    pub arguments: Value,
    /// The tool's original MCP input schema (to validate edits).
    pub schema: Value,
    /// SHA-256 of the payload as proposed.
    pub payload_sha256: String,
}

impl Proposal {
    pub fn new(tool: &str, server: &'static str, arguments: Value, schema: Value) -> Self {
        let payload_sha256 = payload_hash(server, tool, &arguments);
        Self {
            tool: tool.to_string(),
            server,
            arguments,
            schema,
            payload_sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Execute exactly `arguments`, whose hash is `payload_sha256`.
    Approved {
        arguments: Value,
        payload_sha256: String,
        edited: bool,
        reviewer: String,
    },
    Rejected {
        reason: String,
        reviewer: String,
    },
    /// Nobody can approve here (e.g. no console): do not run.
    Unavailable(String),
}

/// SHA-256 over `server \n tool \n canonical JSON`. serde_json objects keep
/// keys sorted, so the same arguments always give the same hash.
pub fn payload_hash(server: &str, tool: &str, arguments: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server.as_bytes());
    hasher.update(b"\n");
    hasher.update(tool.as_bytes());
    hasher.update(b"\n");
    hasher.update(arguments.to_string().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub type DecisionFuture<'a> = Pin<Box<dyn Future<Output = Decision> + Send + 'a>>;

pub trait Approver: Send + Sync {
    fn review<'a>(&'a self, proposal: &'a Proposal) -> DecisionFuture<'a>;
}

// ---------------------------------------------------------------------------
// Console input shared by the REPL and the approver
// ---------------------------------------------------------------------------

pub type LineFuture<'a> = Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>>;

/// Where human input comes from. `None` = end of input.
pub trait LineSource: Send + Sync {
    fn read_line(&self) -> LineFuture<'_>;
}

/// The process's stdin, shareable between the prompt loop and the approver.
#[derive(Clone)]
pub struct SharedStdin(Arc<tokio::sync::Mutex<Lines<BufReader<Stdin>>>>);

impl SharedStdin {
    pub fn new() -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(
            BufReader::new(tokio::io::stdin()).lines(),
        )))
    }
}

impl Default for SharedStdin {
    fn default() -> Self {
        Self::new()
    }
}

impl LineSource for SharedStdin {
    fn read_line(&self) -> LineFuture<'_> {
        Box::pin(async move { self.0.lock().await.next_line().await.ok().flatten() })
    }
}

/// Fixed lines (tests).
#[derive(Default)]
pub struct ScriptedLines(Mutex<VecDeque<String>>);

impl ScriptedLines {
    pub fn new<S: Into<String>>(lines: impl IntoIterator<Item = S>) -> Self {
        Self(Mutex::new(lines.into_iter().map(Into::into).collect()))
    }
}

impl LineSource for ScriptedLines {
    fn read_line(&self) -> LineFuture<'_> {
        let line = self.0.lock().ok().and_then(|mut q| q.pop_front());
        Box::pin(async move { line })
    }
}

// ---------------------------------------------------------------------------
// Console approver
// ---------------------------------------------------------------------------

pub struct ConsoleApprover<S: LineSource> {
    input: S,
}

impl<S: LineSource> ConsoleApprover<S> {
    pub fn new(input: S) -> Self {
        Self { input }
    }

    async fn ask(&self, prompt: &str) -> Option<String> {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        self.input.read_line().await.map(|l| l.trim().to_string())
    }

    async fn decide(&self, proposal: &Proposal) -> Decision {
        let reviewer = "console".to_string();
        let mut arguments = proposal.arguments.clone();
        let mut edited = false;
        loop {
            let hash = payload_hash(proposal.server, &proposal.tool, &arguments);
            let problems = validate(&arguments, &proposal.schema);
            println!("\n  ==================== APPROVAL REQUIRED ====================");
            println!("  Tool:   {}  (server: {})", proposal.tool, proposal.server);
            println!("  This action CHANGES STATE. Exact payload that would be sent:");
            for line in serde_json::to_string_pretty(&arguments)
                .unwrap_or_default()
                .lines()
            {
                println!("  | {line}");
            }
            println!("  Payload SHA-256: {hash}");
            if edited {
                println!("  (edited by you - this new payload needs its own approval)");
            }
            for p in &problems {
                println!("  !! invalid: {p}");
            }
            println!("  ===========================================================");
            let Some(answer) = self
                .ask("  Approve this exact payload? [a]pprove / [r]eject / [e]dit: ")
                .await
            else {
                println!("\n  (no input) - treated as REJECTED");
                return Decision::Rejected {
                    reason: "no approval input (end of input)".into(),
                    reviewer,
                };
            };
            match answer.to_ascii_lowercase().as_str() {
                "a" | "approve" | "y" | "yes" => {
                    if problems.is_empty() {
                        return Decision::Approved {
                            arguments,
                            payload_sha256: hash,
                            edited,
                            reviewer,
                        };
                    }
                    println!("  Cannot approve an invalid payload. Edit it or reject it.");
                }
                "r" | "reject" | "n" | "no" => {
                    let reason = self
                        .ask("  Reason (optional, shown to Claude): ")
                        .await
                        .unwrap_or_default();
                    let reason = if reason.is_empty() {
                        "rejected by the reviewer".to_string()
                    } else {
                        reason
                    };
                    return Decision::Rejected { reason, reviewer };
                }
                "e" | "edit" => {
                    let Some(obj) = arguments.as_object().cloned() else {
                        continue;
                    };
                    println!("  Press Enter to keep a value, or type a new one.");
                    let mut changed = Map::new();
                    for (key, value) in &obj {
                        let shown = value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_string);
                        let Some(new) = self.ask(&format!("  {key} [{shown}]: ")).await else {
                            return Decision::Rejected {
                                reason: "no approval input (end of input)".into(),
                                reviewer,
                            };
                        };
                        if !new.is_empty() {
                            let parsed = match value {
                                Value::Number(_) => new
                                    .parse::<i64>()
                                    .map(Value::from)
                                    .unwrap_or(Value::String(new)),
                                Value::Bool(_) => new
                                    .parse::<bool>()
                                    .map(Value::from)
                                    .unwrap_or(Value::String(new)),
                                _ => Value::String(new),
                            };
                            changed.insert(key.clone(), parsed);
                        }
                    }
                    if !changed.is_empty() {
                        let mut merged = obj;
                        merged.extend(changed);
                        arguments = Value::Object(merged);
                        edited = true;
                    }
                }
                _ => println!("  Please type a, r or e."),
            }
        }
    }
}

impl<S: LineSource> Approver for ConsoleApprover<S> {
    fn review<'a>(&'a self, proposal: &'a Proposal) -> DecisionFuture<'a> {
        Box::pin(self.decide(proposal))
    }
}

// ---------------------------------------------------------------------------
// Scripted and deferred approvers
// ---------------------------------------------------------------------------

/// One pre-programmed reaction.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptedStep {
    Approve,
    Reject(String),
    /// Change these top-level fields, then approve the edited payload.
    EditThenApprove(Value),
    /// Return the hash of the ORIGINAL payload with DIFFERENT arguments,
    /// simulating a modified payload after approval. The agent must refuse.
    Tamper(Value),
}

/// Decisions for tests and evaluations, in order; records what it was shown.
#[derive(Default, Clone)]
pub struct ScriptedApprover {
    steps: Arc<Mutex<VecDeque<ScriptedStep>>>,
    seen: Arc<Mutex<Vec<Proposal>>>,
}

impl ScriptedApprover {
    pub fn new(steps: impl IntoIterator<Item = ScriptedStep>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into_iter().collect())),
            seen: Arc::default(),
        }
    }

    /// Replaces the remaining decisions (e.g. before the next benchmark).
    pub fn set_steps(&self, steps: impl IntoIterator<Item = ScriptedStep>) {
        if let Ok(mut s) = self.steps.lock() {
            *s = steps.into_iter().collect();
        }
    }

    /// Every proposal presented so far.
    pub fn seen(&self) -> Vec<Proposal> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn merge(base: &Value, changes: &Value) -> Value {
    let mut out = base.as_object().cloned().unwrap_or_default();
    if let Some(c) = changes.as_object() {
        out.extend(c.clone());
    }
    Value::Object(out)
}

impl Approver for ScriptedApprover {
    fn review<'a>(&'a self, proposal: &'a Proposal) -> DecisionFuture<'a> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(proposal.clone());
        }
        let step = self.steps.lock().ok().and_then(|mut s| s.pop_front());
        let reviewer = "scripted".to_string();
        let decision = match step {
            Some(ScriptedStep::Approve) => Decision::Approved {
                arguments: proposal.arguments.clone(),
                payload_sha256: proposal.payload_sha256.clone(),
                edited: false,
                reviewer,
            },
            Some(ScriptedStep::Reject(reason)) => Decision::Rejected { reason, reviewer },
            Some(ScriptedStep::EditThenApprove(changes)) => {
                let arguments = merge(&proposal.arguments, &changes);
                let payload_sha256 = payload_hash(proposal.server, &proposal.tool, &arguments);
                Decision::Approved {
                    arguments,
                    payload_sha256,
                    edited: true,
                    reviewer,
                }
            }
            Some(ScriptedStep::Tamper(changes)) => Decision::Approved {
                arguments: merge(&proposal.arguments, &changes),
                payload_sha256: proposal.payload_sha256.clone(),
                edited: false,
                reviewer,
            },
            None => Decision::Rejected {
                reason: "no scripted decision left".into(),
                reviewer,
            },
        };
        Box::pin(async move { decision })
    }
}

/// Approves nothing.
pub struct DeferredApprover;

impl Approver for DeferredApprover {
    fn review<'a>(&'a self, _proposal: &'a Proposal) -> DecisionFuture<'a> {
        Box::pin(async {
            Decision::Unavailable("no human reviewer is available in this mode".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({"type": "object", "additionalProperties": false,
               "required": ["title", "severity"],
               "properties": {"title": {"type": "string", "minLength": 5},
                              "severity": {"type": "string", "enum": ["LOW", "HIGH", "CRITICAL"]}}})
    }

    fn proposal() -> Proposal {
        Proposal::new(
            "telemetry__create_incident_ticket",
            "telemetry",
            json!({"title": "Payments outage", "severity": "CRITICAL"}),
            schema(),
        )
    }

    #[test]
    fn the_hash_is_stable_and_sensitive_to_every_part() {
        let args = json!({"b": 1, "a": "x"});
        let h = payload_hash("telemetry", "t", &args);
        assert_eq!(h.len(), 64);
        assert_eq!(
            h,
            payload_hash("telemetry", "t", &json!({"a": "x", "b": 1})),
            "key order does not matter"
        );
        assert_ne!(
            h,
            payload_hash("telemetry", "t", &json!({"a": "x", "b": 2}))
        );
        assert_ne!(h, payload_hash("files", "t", &args));
        assert_ne!(h, payload_hash("telemetry", "u", &args));
    }

    #[tokio::test]
    async fn console_approve() {
        let p = proposal();
        let d = ConsoleApprover::new(ScriptedLines::new(["a"]))
            .review(&p)
            .await;
        assert_eq!(
            d,
            Decision::Approved {
                arguments: p.arguments.clone(),
                payload_sha256: p.payload_sha256.clone(),
                edited: false,
                reviewer: "console".into()
            }
        );
    }

    #[tokio::test]
    async fn console_reject_with_reason_and_end_of_input() {
        let d = ConsoleApprover::new(ScriptedLines::new(["r", "not enough evidence"]))
            .review(&proposal())
            .await;
        assert_eq!(
            d,
            Decision::Rejected {
                reason: "not enough evidence".into(),
                reviewer: "console".into()
            }
        );
        let d = ConsoleApprover::new(ScriptedLines::new(Vec::<String>::new()))
            .review(&proposal())
            .await;
        assert!(matches!(d, Decision::Rejected { reason, .. } if reason.contains("end of input")));
    }

    #[tokio::test]
    async fn an_edit_needs_a_fresh_approval_of_the_new_payload() {
        // e → keep severity (Enter), new title → payload shown again → a
        let p = proposal();
        let d = ConsoleApprover::new(ScriptedLines::new(["e", "", "Checkout outage", "a"]))
            .review(&p)
            .await;
        let Decision::Approved {
            arguments,
            payload_sha256,
            edited,
            ..
        } = d
        else {
            panic!("{d:?}")
        };
        assert!(edited);
        assert_eq!(
            arguments,
            json!({"title": "Checkout outage", "severity": "CRITICAL"})
        );
        assert_eq!(
            payload_sha256,
            payload_hash("telemetry", "telemetry__create_incident_ticket", &arguments)
        );
        assert_ne!(payload_sha256, p.payload_sha256);

        // An edit followed by end of input is a rejection, not an approval.
        let d = ConsoleApprover::new(ScriptedLines::new(["e", "", "Checkout outage"]))
            .review(&p)
            .await;
        assert!(matches!(d, Decision::Rejected { .. }), "{d:?}");
    }

    #[tokio::test]
    async fn an_invalid_edit_cannot_be_approved() {
        // Edit severity to an invalid value, try to approve (refused), then reject.
        let d = ConsoleApprover::new(ScriptedLines::new(["e", "URGENT", "", "a", "r", ""]))
            .review(&proposal())
            .await;
        assert!(matches!(d, Decision::Rejected { .. }), "{d:?}");
    }

    #[tokio::test]
    async fn unknown_answers_are_asked_again() {
        let d = ConsoleApprover::new(ScriptedLines::new(["maybe", "yes please", "a"]))
            .review(&proposal())
            .await;
        assert!(matches!(d, Decision::Approved { .. }));
    }

    #[tokio::test]
    async fn scripted_steps() {
        let p = proposal();
        let s = ScriptedApprover::new([
            ScriptedStep::Approve,
            ScriptedStep::Reject("no".into()),
            ScriptedStep::EditThenApprove(json!({"severity": "HIGH"})),
            ScriptedStep::Tamper(json!({"severity": "LOW"})),
        ]);
        assert!(matches!(
            s.review(&p).await,
            Decision::Approved { edited: false, .. }
        ));
        assert!(matches!(s.review(&p).await, Decision::Rejected { .. }));
        let Decision::Approved {
            arguments,
            payload_sha256,
            ..
        } = s.review(&p).await
        else {
            panic!()
        };
        assert_eq!(arguments["severity"], "HIGH");
        assert_eq!(payload_sha256, payload_hash(p.server, &p.tool, &arguments));
        let Decision::Approved {
            arguments,
            payload_sha256,
            ..
        } = s.review(&p).await
        else {
            panic!()
        };
        assert_ne!(
            payload_sha256,
            payload_hash(p.server, &p.tool, &arguments),
            "tamper keeps the old hash"
        );
        assert!(
            matches!(s.review(&p).await, Decision::Rejected { .. }),
            "script exhausted → reject"
        );
        assert_eq!(s.seen().len(), 5);
    }
}
