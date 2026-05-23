//! `OpenAI` schema family for the caliban agent harness.
//!
//! Provides [`OpenAIProvider<T: Transport>`] generic over its transport.
//! Direct API is supported by default; Azure `OpenAI` transport is gated
//! behind the `azure` cargo feature.

#![allow(clippy::missing_errors_doc)]
// Transitive dependencies pull in multiple versions of some crates.
#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod error;
pub mod ir_convert;
pub mod models;
pub mod schema;
pub mod transport;

mod stream_parse; // populated in Task 5

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, Error, MessageStream, ModelInfo, Provider,
    Result,
};

use crate::config::DirectConfig;
use crate::transport::Transport;
use crate::transport::direct::DirectTransport;

/// `OpenAI` Chat Completions provider, generic over its transport.
pub struct OpenAIProvider<T: Transport> {
    transport: T,
}

impl<T: Transport> std::fmt::Debug for OpenAIProvider<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIProvider").finish_non_exhaustive()
    }
}

impl OpenAIProvider<DirectTransport> {
    /// Construct an `OpenAIProvider` using the direct HTTPS transport.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn direct(cfg: DirectConfig) -> Result<Self> {
        DirectTransport::new(cfg)
            .map(|t| Self { transport: t })
            .map_err(Error::adapter)
    }
}

impl<T: Transport> OpenAIProvider<T> {
    /// Construct an `OpenAIProvider` from an arbitrary `Transport`.
    pub fn from_transport(transport: T) -> Self {
        Self { transport }
    }
}

#[cfg(feature = "azure")]
impl OpenAIProvider<crate::transport::azure::AzureTransport> {
    /// Construct an `OpenAIProvider` using the Azure `OpenAI` Service transport.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn azure(cfg: crate::config::AzureConfig) -> Result<Self> {
        crate::transport::azure::AzureTransport::new(cfg)
            .map(|t| Self { transport: t })
            .map_err(Error::adapter)
    }
}

#[async_trait]
impl<T: Transport> Provider for OpenAIProvider<T> {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let mut native = ir_convert::ir_to_native_request(req, false)?;
        native.model = self.transport.wire_model_id(&canonical_model);
        self.transport.finalize_request(&mut native);
        let native_resp = self.transport.send(native).await.map_err(Error::from)?;
        ir_convert::native_response_to_ir(native_resp)
    }

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let mut native = ir_convert::ir_to_native_request(req, true)?;
        native.model = self.transport.wire_model_id(&canonical_model);
        // Opt into usage reporting on the final streaming chunk.
        native.stream_options = Some(crate::schema::request::NativeStreamOptions {
            include_usage: true,
        });
        self.transport.finalize_request(&mut native);
        let bytes_stream = self
            .transport
            .stream(native)
            .await
            .map_err(caliban_provider::Error::from)?;
        Ok(stream_parse::map_openai_sse_to_events(bytes_stream))
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        models::capabilities_for(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        models::models()
    }

    fn name(&self) -> &'static str {
        "openai"
    }
}
