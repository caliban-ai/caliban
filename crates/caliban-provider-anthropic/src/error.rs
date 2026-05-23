//! Adapter-internal error type and conversion to `caliban_provider::Error`.

use caliban_provider::Error as ProviderError;

/// Errors produced internally by the Anthropic adapter.
#[derive(thiserror::Error, Debug)]
pub enum AnthropicError {
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
    },

    /// JSON deserialization failed.
    #[error("deserialize error: {0}")]
    Deserialize(#[from] serde_json::Error),

    /// SSE stream parse failed.
    #[error("stream parse error: {0}")]
    StreamParse(String),

    /// A required environment-variable or config field was absent.
    #[error("missing config field: {0}")]
    MissingConfig(&'static str),

    /// A generic transport-level error.
    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),
}

impl From<AnthropicError> for ProviderError {
    fn from(e: AnthropicError) -> Self {
        match e {
            AnthropicError::Http(ref reqwest_err) => {
                if reqwest_err.is_connect() || reqwest_err.is_timeout() {
                    ProviderError::network(e)
                } else {
                    ProviderError::adapter(e)
                }
            }
            AnthropicError::BadStatus { status, ref body } => match status {
                401 | 403 => ProviderError::Auth(body.clone()),
                429 => ProviderError::RateLimit { retry_after: None },
                400 => ProviderError::InvalidRequest(body.clone()),
                404 => ProviderError::ModelUnavailable(body.clone()),
                _ if status >= 500 => ProviderError::ServerError {
                    status,
                    body: body.clone(),
                },
                _ => ProviderError::adapter(e),
            },
            AnthropicError::Deserialize(_)
            | AnthropicError::StreamParse(_)
            | AnthropicError::MissingConfig(_)
            | AnthropicError::Transport(_) => ProviderError::adapter(e),
        }
    }
}
