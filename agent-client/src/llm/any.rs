//! One client type that is either Anthropic or OpenRouter, so `Agent<AnyClient>`
//! stays a single concrete type and no other code has to become provider-aware.

use super::{
    AnthropicClient, LlmClient, LlmError, MessageResponse, MessagesRequest, OpenRouterClient,
};

pub enum AnyClient {
    Anthropic(AnthropicClient),
    OpenRouter(OpenRouterClient),
}

// NOTE: the signature below must match the `LlmClient` trait in anthropic.rs exactly.
// If that trait uses `#[async_trait]`, add `#[async_trait::async_trait]` above this impl.
impl LlmClient for AnyClient {
    async fn send(&self, request: &MessagesRequest) -> Result<MessageResponse, LlmError> {
        match self {
            Self::Anthropic(client) => client.send(request).await,
            Self::OpenRouter(client) => client.send(request).await,
        }
    }
}
