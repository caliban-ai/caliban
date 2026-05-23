//! The `Transport` trait — abstracts how an Ollama request is delivered.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::OllamaError;
use crate::schema::{NativeRequest, NativeResponse};

/// Abstraction over how Ollama `/api/chat` requests are sent.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Send a non-streaming request and return the parsed response.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError)` on network failure, non-2xx HTTP status,
    /// or deserialization failure.
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, OllamaError>;

    /// Send a streaming request and return a byte stream of NDJSON chunks.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError)` on network failure or non-2xx HTTP status.
    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, OllamaError>>, OllamaError>;

    /// Map a canonical model ID to the wire model ID for this transport.
    fn wire_model_id(&self, canonical: &str) -> String {
        canonical.to_string()
    }

    /// Apply any transport-specific mutations to the request before sending.
    fn finalize_request(&self, _body: &mut NativeRequest) {}
}

pub mod direct;
