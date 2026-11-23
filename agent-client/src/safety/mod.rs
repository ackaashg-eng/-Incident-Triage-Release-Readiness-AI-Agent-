//! Security layers that do not depend on the model behaving well.
//!
//! * `policy` (in `agent::registry`): trusted, application-side tool classification
//! * `agent::approval`: human approval bound to the exact payload (SHA-256)
//! * `injection`: flags instruction-like text and labels tool output as untrusted data
//! * `redact`: removes secrets from tool output, traces and console echoes
//! * `mcp::filesystem` / `mcp::child_env`: confinement and a scrubbed child environment

pub mod injection;
pub mod redact;
