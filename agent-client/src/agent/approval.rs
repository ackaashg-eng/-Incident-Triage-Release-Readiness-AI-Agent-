//! The hand-off point for write actions that need a human decision.
//!
//! When Claude calls a tool classified `RequiresApproval` (today:
//! `telemetry__create_incident_ticket`), the agent does NOT execute it. It
//! builds a [`Proposal`] and asks the [`Approver`]. Phase 5 ships only
//! [`DeferredApprover`], which shows the proposal and declines to run it.
//! Phase 6 adds the interactive Approve / Edit / Reject approver.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// A write action Claude asked for, exactly as it would be executed.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// Namespaced tool name, e.g. `telemetry__create_incident_ticket`.
    pub tool: String,
    pub server: &'static str,
    /// Arguments, already validated against the tool's schema.
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Run it with these arguments (possibly edited by the human).
    Approved(Value),
    /// The human said no (reason shown to Claude).
    Rejected(String),
    /// Nobody can approve in this build/mode: do not run.
    Unavailable(String),
}

pub type DecisionFuture<'a> = Pin<Box<dyn Future<Output = Decision> + Send + 'a>>;

pub trait Approver: Send + Sync {
    fn review<'a>(&'a self, proposal: &'a Proposal) -> DecisionFuture<'a>;
}

/// Phase 5: approval is not implemented yet, so nothing that needs it runs.
pub struct DeferredApprover;

impl Approver for DeferredApprover {
    fn review<'a>(&'a self, _proposal: &'a Proposal) -> DecisionFuture<'a> {
        Box::pin(async {
            Decision::Unavailable(
                "human approval is not available yet (the approval gate arrives in Phase 6)"
                    .to_string(),
            )
        })
    }
}
