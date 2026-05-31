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

    /// The upstream model server reported an internal fault (process
    /// crash, OOM kill, segfault, etc.) — distinct from `ServerError`
    /// because the fault may arrive in-band (HTTP 200 + SSE error
    /// payload, as LM Studio does when the model crashes mid-stream).
    /// The fault is server-side, not request-side, so callers should
    /// surface it as such rather than as `InvalidRequest`.
    #[error("upstream server fault: {0}")]
    UpstreamServerFault(String),

    /// The response was blocked by a content-safety filter.
    #[error("content filter triggered: {0}")]
    ContentFilter(String),

    /// A transport-level network error occurred.
    #[error("network error: {0}")]
    Network(Box<dyn std::error::Error + Send + Sync>),

    /// The HTTP response body was severed mid-stream. Distinct from
    /// `Network` because the request itself succeeded (the upstream
    /// accepted it and started replying) — what failed was reading the
    /// streaming body to completion. Typical triggers: TCP RST or FIN
    /// from upstream while the SSE stream was in flight, idle teardown
    /// by NAT/proxy, or a transient connection reset. The wrapped
    /// string is the underlying transport error chain, captured at the
    /// point the chunk read failed.
    #[error("stream interrupted mid-response: {0}")]
    StreamInterrupted(String),

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

    /// Wrap an upstream-severed-stream error. `inner` is rendered into
    /// the message so the user-visible line reads "stream interrupted
    /// mid-response: <source chain>".
    pub fn stream_interrupted(inner: impl std::fmt::Display) -> Self {
        Self::StreamInterrupted(inner.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_interrupted_display_uses_clear_prefix() {
        // Operator-visible line. Must not say "decoding response body"
        // (the prior misleading reqwest phrasing); must say something a
        // user can recognize as a transport-level cutoff.
        let e = Error::stream_interrupted("hyper: connection reset by peer");
        assert_eq!(
            e.to_string(),
            "stream interrupted mid-response: hyper: connection reset by peer"
        );
    }

    #[test]
    fn stream_interrupted_constructor_accepts_display() {
        // Helper should accept anything Display-able, mirroring how
        // `network` / `adapter` accept anything Error-able.
        let io = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof");
        let e = Error::stream_interrupted(io);
        assert!(matches!(e, Error::StreamInterrupted(_)));
        assert!(e.to_string().contains("eof"));
    }
}
