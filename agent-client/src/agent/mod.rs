//! The agent: tool registry, argument validation, system prompt, the
//! approval hand-off and the agentic loop.

pub mod approval;
pub mod prompt;
pub mod registry;
pub mod runner;
pub mod validate;

pub use runner::{
    Agent, AgentEvent, AgentSettings, ApprovalStatus, Fanout, NoopObserver, Observer,
    ToolCallRecord, ToolStatus, TurnOutcome, TurnReport,
};
