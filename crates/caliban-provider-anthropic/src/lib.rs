//! Anthropic Claude schema family for the caliban agent harness.
//!
//! Provides [`AnthropicProvider<T: Transport>`] generic over its transport.
//! Direct API is supported by default; AWS Bedrock and Google Vertex AI
//! transports are gated behind cargo features (`bedrock`, `vertex`).

#![allow(clippy::missing_errors_doc)]
// Transitive dependencies (reqwest, hyper, etc.) pull in multiple versions of
// windows-sys, thiserror, etc. These are not under our control.
#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod error;
pub mod ir_convert;
pub mod models;
pub mod schema;
pub mod transport;

mod stream_parse; // populated in Task 3

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, Error, MessageStream, ModelInfo, Provider,
    Result,
};

use crate::config::DirectConfig;
use crate::transport::Transport;
use crate::transport::direct::DirectTransport;

/// Anthropic Claude provider, generic over its transport.
pub struct AnthropicProvider<T: Transport> {
    transport: T,
}

impl AnthropicProvider<DirectTransport> {
    /// Construct an `AnthropicProvider` using the direct HTTPS transport.
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

impl<T: Transport> AnthropicProvider<T> {
    /// Construct an `AnthropicProvider` from an arbitrary `Transport`.
    pub fn from_transport(transport: T) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl<T: Transport> Provider for AnthropicProvider<T> {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let mut native = ir_convert::ir_to_native_request(req, false);
        native.model = self.transport.wire_model_id(&canonical_model);
        self.transport.finalize_request(&mut native);
        let native_resp = self.transport.send(native).await.map_err(Error::from)?;
        ir_convert::native_response_to_ir(native_resp)
    }

    async fn stream(&self, _req: CompletionRequest) -> Result<MessageStream> {
        Err(Error::InvalidRequest(
            "Anthropic streaming not yet wired (see Task 3)".into(),
        ))
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        models::capabilities_for(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        models::models()
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }
}
