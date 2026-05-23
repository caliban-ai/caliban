//! The `Transport` trait — abstracts how an `OpenAI` request is delivered.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::OpenAIError;
use crate::schema::{NativeRequest, NativeResponse};

/// Abstraction over how `OpenAI` Chat Completions requests are sent (direct HTTPS, Azure, etc.).
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Send a non-streaming request and return the parsed response.
    ///
    /// # Errors
    ///
    /// Returns `Err(OpenAIError)` on network failure, non-2xx HTTP status,
    /// or deserialization failure.
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, OpenAIError>;

    /// Send a streaming request and return a byte stream of SSE chunks.
    ///
    /// # Errors
    ///
    /// Returns `Err(OpenAIError)` on network failure or non-2xx HTTP status.
    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, OpenAIError>>, OpenAIError>;

    /// Map a canonical model ID to the wire model ID for this transport.
    fn wire_model_id(&self, canonical: &str) -> String {
        canonical.to_string()
    }

    /// Apply any transport-specific mutations to the request before sending.
    fn finalize_request(&self, _body: &mut NativeRequest) {}
}

pub mod direct;

#[cfg(feature = "azure")]
pub mod azure;
