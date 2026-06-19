//! Google Gemini schema family for the caliban agent harness.
//!
//! Provides [`GoogleProvider<T: Transport>`] generic over its transport.
//! The AI Studio transport is always available. The Vertex AI transport
//! is gated behind the `vertex` cargo feature (added in B.10).

#![allow(clippy::missing_errors_doc)]
// Transitive dependencies pull in multiple versions of some crates.
#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod error;
pub mod ir_convert;
pub mod models;
pub mod schema;
pub mod transport;

mod stream_parse;

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, Error, MessageStream, ModelInfo, Provider,
    Result,
};

use crate::config::AIStudioConfig;
use crate::transport::Transport;
use crate::transport::ai_studio::AIStudioTransport;

/// Google Gemini provider, generic over its transport.
pub struct GoogleProvider<T: Transport> {
    transport: T,
}

impl<T: Transport> std::fmt::Debug for GoogleProvider<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleProvider").finish_non_exhaustive()
    }
}

impl GoogleProvider<AIStudioTransport> {
    /// Construct a `GoogleProvider` using the Google AI Studio HTTPS transport.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn ai_studio(cfg: AIStudioConfig) -> Result<Self> {
        AIStudioTransport::new(cfg)
            .map(|t| Self { transport: t })
            .map_err(Error::adapter)
    }
}

impl<T: Transport> GoogleProvider<T> {
    /// Construct a `GoogleProvider` from an arbitrary `Transport`.
    pub fn from_transport(transport: T) -> Self {
        Self { transport }
    }
}

#[cfg(feature = "vertex")]
impl GoogleProvider<crate::transport::vertex::VertexTransport> {
    /// Construct a `GoogleProvider` using the Google Vertex AI transport.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn vertex(cfg: crate::config::VertexConfig) -> Result<Self> {
        crate::transport::vertex::VertexTransport::new(cfg)
            .map(|t| Self { transport: t })
            .map_err(Error::adapter)
    }
}

#[async_trait]
impl<T: Transport> Provider for GoogleProvider<T> {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let wire_model = self.transport.wire_model_id(&canonical_model);
        let allow_urls = self.transport.supports_url_images();
        let native = ir_convert::ir_to_native_request(req, allow_urls)?;
        let native_resp = self
            .transport
            .send(&wire_model, native)
            .await
            .map_err(Error::from)?;
        ir_convert::native_response_to_ir(native_resp)
    }

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let wire_model = self.transport.wire_model_id(&canonical_model);
        let allow_urls = self.transport.supports_url_images();
        let native = ir_convert::ir_to_native_request(req, allow_urls)?;
        let bytes_stream = self
            .transport
            .stream(&wire_model, native)
            .await
            .map_err(Error::from)?;
        Ok(stream_parse::map_gemini_sse_to_events(bytes_stream))
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        models::capabilities_for(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        models::models()
    }

    fn name(&self) -> &'static str {
        "google"
    }
}
