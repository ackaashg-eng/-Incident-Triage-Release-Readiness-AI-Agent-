//! Claude integration (Anthropic Messages API over raw HTTPS).
//!
//! * `types`: request body construction and response parsing
//! * `anthropic`: the HTTP client (timeouts, retries, errors) and the
//!   `LlmClient` trait the agent depends on (so tests can script Claude)

pub mod anthropic;
mod any;
pub mod openrouter;
pub mod types;

pub use anthropic::{AnthropicClient, LlmClient, LlmError};
pub use any::AnyClient;
pub use openrouter::OpenRouterClient;
pub use types::{MessageResponse, MessagesRequest, StopReason, ToolDefinition, ToolUse};
