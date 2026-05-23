//! The `Provider` trait.

use async_trait::async_trait;

use crate::capabilities::{Capabilities, ModelInfo};
use crate::error::Result;
use crate::request::CompletionRequest;
use crate::response::CompletionResponse;
use crate::stream::MessageStream;

/// Object-safe async trait implemented by each provider adapter.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Send a blocking (non-streaming) completion request.
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse>;

    /// Send a streaming completion request.
    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream>;

    /// Return the capabilities of the given model as reported by this provider.
    fn capabilities(&self, model: &str) -> Capabilities;

    /// List all models supported by this provider.
    fn list_models(&self) -> Vec<ModelInfo>;

    /// A short, stable name identifying this provider (e.g. `"anthropic"`).
    fn name(&self) -> &'static str;
}
