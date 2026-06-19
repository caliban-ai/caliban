//! Adapter-internal error type and conversion to `caliban_provider::Error`.

use caliban_provider::Error as ProviderError;

/// Errors produced internally by the Ollama adapter.
#[derive(thiserror::Error, Debug)]
pub enum OllamaError {
    /// An HTTP transport failure from `reqwest`.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// A non-2xx HTTP response with its status code and body.
    #[error("response status {status}: {body}")]
    BadStatus {
        /// The HTTP status code.
        status: u16,
        /// The response body text.
        body: String,
        /// The parsed `Retry-After` hint, when the server sent one (429s).
        retry_after: Option<std::time::Duration>,
    },

    /// JSON deserialization failed.
    #[error("deserialize error: {0}")]
    Deserialize(#[from] serde_json::Error),

    /// NDJSON stream parse failed.
    #[error("stream parse error: {0}")]
    StreamParse(String),

    /// A required environment-variable or config field was absent.
    #[error("missing config field: {0}")]
    MissingConfig(&'static str),

    /// A generic transport-level error.
    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),

    /// An unsupported feature was requested.
    #[error("unsupported feature: {0}")]
    Unsupported(String),
}

impl OllamaError {
    /// Build [`OllamaError::BadStatus`] from the shared
    /// [`caliban_provider::transport::BadResponse`] that
    /// [`caliban_provider::transport::check_status`] produces — the single
    /// place this adapter turns a non-2xx response (status, body, and parsed
    /// `Retry-After`) into its error variant.
    #[must_use]
    pub(crate) fn bad_status(resp: caliban_provider::transport::BadResponse) -> Self {
        Self::BadStatus {
            status: resp.status,
            body: resp.body,
            retry_after: resp.retry_after,
        }
    }
}

impl From<OllamaError> for ProviderError {
    fn from(e: OllamaError) -> Self {
        use caliban_provider::TransportErrorClass;
        match e {
            OllamaError::Http(ref err) => {
                match caliban_provider::classify_reqwest_error("ollama", err) {
                    TransportErrorClass::StreamInterrupted => ProviderError::stream_interrupted(
                        caliban_provider::render_source_chain(err),
                    ),
                    TransportErrorClass::Network => ProviderError::network(e),
                    TransportErrorClass::Adapter => ProviderError::adapter(e),
                }
            }
            OllamaError::BadStatus {
                status,
                ref body,
                retry_after,
            } => caliban_provider::error_classify::map_bad_status(status, body, retry_after, |b| {
                ProviderError::InvalidRequest(b.to_string())
            })
            .unwrap_or_else(|| ProviderError::adapter(e)),
            OllamaError::Deserialize(_)
            | OllamaError::StreamParse(_)
            | OllamaError::MissingConfig(_)
            | OllamaError::Transport(_)
            | OllamaError::Unsupported(_) => ProviderError::adapter(e),
        }
    }
}
