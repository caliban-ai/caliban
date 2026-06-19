//! The `Transport` trait — abstracts how a Gemini request is delivered.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::GoogleError;
use crate::schema::{NativeRequest, NativeResponse};

/// Abstraction over how Gemini requests are sent (AI Studio, Vertex AI, etc.).
///
/// `send`/`stream` take the request **by value**, matching the other adapters'
/// `Transport` traits — the owned-vs-borrowed split was accidental, not
/// essential. The `model` parameter is retained (unlike the other adapters,
/// which carry the model inside the request body): Gemini routes by model in
/// the request *URL* (`…/models/{model}:generateContent`), so the transport
/// genuinely needs it. `supports_url_images` is likewise essential — it drives
/// request conversion at the provider layer (AI Studio inlines images, Vertex
/// references them by URI).
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Send a non-streaming request and return the parsed response.
    ///
    /// # Errors
    ///
    /// Returns `Err(GoogleError)` on network failure, non-2xx HTTP status,
    /// or deserialization failure.
    async fn send(&self, model: &str, body: NativeRequest) -> Result<NativeResponse, GoogleError>;

    /// Send a streaming request and return a byte stream of SSE chunks.
    ///
    /// # Errors
    ///
    /// Returns `Err(GoogleError)` on network failure or non-2xx HTTP status.
    async fn stream(
        &self,
        model: &str,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, GoogleError>>, GoogleError>;

    /// Map a canonical model ID to the wire model ID for this transport.
    fn wire_model_id(&self, canonical: &str) -> String {
        canonical.to_string()
    }

    /// Whether this transport accepts URL images (as `fileData` parts).
    ///
    /// AI Studio requires base64-inline images; Vertex AI supports URI references.
    /// Defaults to `false`.
    fn supports_url_images(&self) -> bool {
        false
    }
}

pub mod ai_studio;

#[cfg(feature = "vertex")]
pub mod vertex;
