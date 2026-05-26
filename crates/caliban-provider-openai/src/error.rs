//! Adapter-internal error type and conversion to `caliban_provider::Error`.

use caliban_provider::Error as ProviderError;

/// Errors produced internally by the `OpenAI` adapter.
#[derive(thiserror::Error, Debug)]
pub enum OpenAIError {
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

    /// The upstream server returned a JSON error object in the SSE body
    /// (rather than as a non-2xx HTTP status). Observed against LM Studio
    /// when, e.g., the request exceeds the loaded context window — the
    /// server replies with HTTP 200 and an `{"error": {"message": ...}}`
    /// payload in the stream body. Surfacing the upstream message
    /// verbatim avoids the layered "stream parse / chunk parse / missing
    /// field 'id'" wrapping that the bare deserialization error produces.
    /// See `docs/2026-05-25-lmstudio-probe-findings.md` Finding 12.
    #[error("upstream error: {0}")]
    UpstreamError(String),

    /// A required environment-variable or config field was absent.
    #[error("missing config field: {0}")]
    MissingConfig(String),

    /// A generic transport-level error.
    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),

    /// An unsupported feature was requested.
    #[error("unsupported feature: {0}")]
    Unsupported(String),
}

impl From<OpenAIError> for ProviderError {
    fn from(e: OpenAIError) -> Self {
        match e {
            OpenAIError::Http(ref err) => {
                if err.is_connect() || err.is_timeout() {
                    ProviderError::network(e)
                } else {
                    ProviderError::adapter(e)
                }
            }
            OpenAIError::BadStatus { status, ref body } => match status {
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
            OpenAIError::Deserialize(_)
            | OpenAIError::StreamParse(_)
            | OpenAIError::MissingConfig(_)
            | OpenAIError::Transport(_)
            | OpenAIError::Unsupported(_) => ProviderError::adapter(e),
            // Upstream-reported errors (in-band SSE error payload) are
            // request-shaped problems most of the time (oversized prompt,
            // missing model, malformed input). Map to InvalidRequest so
            // the surface line reads cleanly; the message is the upstream
            // text verbatim.
            OpenAIError::UpstreamError(ref msg) => ProviderError::InvalidRequest(msg.clone()),
        }
    }
}
