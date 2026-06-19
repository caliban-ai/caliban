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
    ///
    /// This is the synchronous, offline view — a static catalog or a cached
    /// snapshot. Providers whose real model set or limits are only knowable at
    /// runtime expose that through [`Provider::refresh_models`].
    fn list_models(&self) -> Vec<ModelInfo>;

    /// Discover this provider's model list, hitting the backend when the real
    /// set or its limits are only knowable at runtime (e.g. Vertex's live
    /// publisher list, Ollama's server-detected context windows).
    ///
    /// The default returns the static [`Provider::list_models`] catalog, so
    /// providers with a fixed model set need not override it. Live providers
    /// override this to fold what would otherwise be a side-channel method into
    /// the trait contract — it is the hook a runtime model-list refresh (#34)
    /// drives, so callers get real data through the trait rather than via
    /// per-adapter back doors.
    ///
    /// # Errors
    ///
    /// Returns an error if the live discovery call fails (network, auth, or a
    /// non-success response). Callers may fall back to [`Provider::list_models`].
    async fn refresh_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self.list_models())
    }

    /// A short, stable name identifying this provider (e.g. `"anthropic"`).
    fn name(&self) -> &'static str;
}
