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
        /// The parsed `Retry-After` hint, when the server sent one (429s).
        retry_after: Option<std::time::Duration>,
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

/// Context-window-overflow markers. Anthropic's shape is
/// `{"error":{"type":"invalid_request_error","message":"prompt is too long: X tokens > Y maximum"}}`.
const CONTEXT_MARKERS: &[&str] = &[
    "prompt is too long",
    "context window",
    "maximum context length",
    "context_length_exceeded",
];

/// Token-count markers for the "prompt is too long: 210000 tokens > 200000
/// maximum" phrasing — the requested count follows "too long: ", the max
/// follows "> ".
const MAX_TOKEN_MARKERS: &[&str] = &["> "];
const REQUESTED_TOKEN_MARKERS: &[&str] = &["too long: "];

impl AnthropicError {
    /// Build [`AnthropicError::BadStatus`] from the shared
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

impl From<AnthropicError> for ProviderError {
    fn from(e: AnthropicError) -> Self {
        use caliban_provider::TransportErrorClass;
        match e {
            AnthropicError::Http(ref err) => {
                match caliban_provider::classify_reqwest_error("anthropic", err) {
                    TransportErrorClass::StreamInterrupted => ProviderError::stream_interrupted(
                        caliban_provider::render_source_chain(err),
                    ),
                    TransportErrorClass::Network => ProviderError::network(e),
                    TransportErrorClass::Adapter => ProviderError::adapter(e),
                }
            }
            // Anthropic 400s are request-shaped; only a context-window overflow
            // is reclassified (→ ContextTooLong) so reactive compaction fires.
            // No upstream-fault classifier here (the first-party API does not
            // surface model-process crashes as 400 bodies).
            AnthropicError::BadStatus {
                status,
                ref body,
                retry_after,
            } => caliban_provider::error_classify::map_bad_status(status, body, retry_after, |b| {
                caliban_provider::error_classify::classify_context_too_long(
                    b,
                    CONTEXT_MARKERS,
                    MAX_TOKEN_MARKERS,
                    REQUESTED_TOKEN_MARKERS,
                )
                .unwrap_or_else(|| ProviderError::InvalidRequest(b.to_string()))
            })
            .unwrap_or_else(|| ProviderError::adapter(e)),
            AnthropicError::Deserialize(_)
            | AnthropicError::StreamParse(_)
            | AnthropicError::MissingConfig(_)
            | AnthropicError::Transport(_) => ProviderError::adapter(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_prompt_too_long_extracts_token_counts() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#;
        let e = AnthropicError::BadStatus {
            status: 400,
            body: body.to_string(),
            retry_after: None,
        };
        match ProviderError::from(e) {
            ProviderError::ContextTooLong {
                max_tokens,
                requested_tokens,
            } => {
                assert_eq!(requested_tokens, 210_000);
                assert_eq!(max_tokens, 200_000);
            }
            other => panic!("expected ContextTooLong, got {other:?}"),
        }
    }

    #[test]
    fn non_context_400_stays_invalid_request() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"invalid messages: empty content block"}}"#;
        let e = AnthropicError::BadStatus {
            status: 400,
            body: body.to_string(),
            retry_after: None,
        };
        match ProviderError::from(e) {
            ProviderError::InvalidRequest(s) => assert!(s.contains("invalid messages")),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }
}
