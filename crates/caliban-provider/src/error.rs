//! Cross-provider error enum.

use std::time::Duration;

/// All errors that can be produced by a caliban provider.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Authentication credentials were missing or rejected.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The provider rejected the request due to rate limiting.
    #[error("rate limit exceeded (retry after {retry_after:?})")]
    RateLimit {
        /// How long to wait before retrying, if known.
        retry_after: Option<Duration>,
    },

    /// The request was structurally invalid.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The request exceeds the model's context window.
    #[error("context too long: requested {requested_tokens} but max is {max_tokens}")]
    ContextTooLong {
        /// The model's maximum context size.
        max_tokens: u32,
        /// The number of tokens in the request.
        requested_tokens: u32,
    },

    /// The requested model is not available via this provider.
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),

    /// The provider returned an HTTP error response.
    #[error("server error (HTTP {status}): {body}")]
    ServerError {
        /// HTTP status code.
        status: u16,
        /// Response body text.
        body: String,
    },

    /// The response was blocked by a content-safety filter.
    #[error("content filter triggered: {0}")]
    ContentFilter(String),

    /// A transport-level network error occurred.
    #[error("network error: {0}")]
    Network(Box<dyn std::error::Error + Send + Sync>),

    /// The operation was cancelled before completion.
    #[error("operation cancelled")]
    Cancelled,

    /// An adapter-specific error that does not fit other categories.
    #[error("adapter error: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The streaming response went silent past the idle timeout.
    #[error("stream idle for {0:?}")]
    StreamIdle(std::time::Duration),
}

/// Convenience alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Wrap a network-layer error.
    pub fn network(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Network(Box::new(e))
    }

    /// Wrap an adapter-specific error.
    pub fn adapter(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Adapter(Box::new(e))
    }
}
