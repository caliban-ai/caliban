//! The `Transport` trait — abstracts how a Gemini request is delivered.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::GoogleError;
use crate::schema::{NativeRequest, NativeResponse};

/// Abstraction over how Gemini requests are sent (AI Studio, Vertex AI, etc.).
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Send a non-streaming request and return the parsed response.
    ///
    /// # Errors
    ///
    /// Returns `Err(GoogleError)` on network failure, non-2xx HTTP status,
    /// or deserialization failure.
    async fn send(&self, model: &str, body: &NativeRequest) -> Result<NativeResponse, GoogleError>;

    /// Send a streaming request and return a byte stream of SSE chunks.
    ///
    /// # Errors
    ///
    /// Returns `Err(GoogleError)` on network failure or non-2xx HTTP status.
    async fn stream(
        &self,
        model: &str,
        body: &NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, GoogleError>>, GoogleError>;

    /// Map a canonical model ID to the wire model ID for this transport.
    fn wire_model_id(&self, canonical: &str) -> String {
        canonical.to_string()
    }
}

pub mod ai_studio;
