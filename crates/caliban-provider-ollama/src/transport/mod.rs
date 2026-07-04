//! The `Transport` trait — abstracts how an Ollama request is delivered.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::OllamaError;
use crate::schema::{ModelShow, NativeRequest, NativeResponse, RunningModel, TagEntry};

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

    /// A stable identifier for the server this transport talks to, used to key
    /// the persisted discovery cache (#316). Different servers cache
    /// independently. The default is a single shared key; `DirectTransport`
    /// returns `host:port`.
    fn server_id(&self) -> String {
        "default".to_string()
    }

    /// Apply any transport-specific mutations to the request before sending.
    fn finalize_request(&self, _body: &mut NativeRequest) {}

    /// List the models the server currently has loaded (`GET /api/ps`).
    ///
    /// Used for runtime context-window detection: a loaded model reports its
    /// live `context_length`. The default returns an empty list so transports
    /// that cannot probe (mocks, alternative back ends) simply fall through to
    /// the static capability table.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError)` on network failure, non-2xx status, or a
    /// deserialization failure. Callers treat any error as "no data" and fall
    /// back to the next detection source.
    async fn running_models(&self) -> Result<Vec<RunningModel>, OllamaError> {
        Ok(Vec::new())
    }

    /// Fetch model metadata (`POST /api/show`), which carries the model's
    /// maximum context length even when it is not loaded.
    ///
    /// The default returns `None` for non-probing transports.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError)` on network failure, non-2xx status, or a
    /// deserialization failure. Callers treat any error as "no data".
    async fn show_model(&self, _model: &str) -> Result<Option<ModelShow>, OllamaError> {
        Ok(None)
    }

    /// List the models the server has available/pulled (`GET /api/tags`).
    ///
    /// Backs runtime model discovery (#316): the returned names are the wire
    /// ids the picker offers. The default returns an empty list so non-probing
    /// transports (mocks, alternative back ends) fall through to the persisted
    /// discovery cache.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError)` on network failure, non-2xx status, or a
    /// deserialization failure. Callers treat any error as "no data".
    async fn list_tags(&self) -> Result<Vec<TagEntry>, OllamaError> {
        Ok(Vec::new())
    }
}

pub mod direct;
