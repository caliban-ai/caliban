//! Ollama schema family for the caliban agent harness.
//!
//! Provides [`OllamaProvider<T: Transport>`] generic over its transport.
//! Default `DirectTransport` talks to a local Ollama instance at
//! `http://localhost:11434`. No authentication is required.

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

use crate::config::DirectConfig;
use crate::transport::Transport;
use crate::transport::direct::DirectTransport;

/// Ollama `/api/chat` provider, generic over its transport.
#[derive(Debug)]
pub struct OllamaProvider<T: Transport> {
    transport: T,
}

impl OllamaProvider<DirectTransport> {
    /// Construct an `OllamaProvider` using the direct HTTP transport.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn direct(cfg: DirectConfig) -> Result<Self> {
        DirectTransport::new(cfg)
            .map(Self::from_transport)
            .map_err(Error::adapter)
    }

    /// Construct an `OllamaProvider` targeting a local Ollama instance with default settings.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn local() -> Result<Self> {
        Self::direct(DirectConfig::local())
    }
}

impl<T: Transport> OllamaProvider<T> {
    /// Construct an `OllamaProvider` from an arbitrary `Transport`.
    pub fn from_transport(transport: T) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl<T: Transport> Provider for OllamaProvider<T> {
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
        self.transport.finalize_request(&mut native);
        let bytes_stream = self.transport.stream(native).await.map_err(Error::from)?;
        Ok(stream_parse::map_ndjson_to_events(bytes_stream))
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        models::capabilities_for(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        models::models()
    }

    fn name(&self) -> &'static str {
        "ollama"
    }
}
