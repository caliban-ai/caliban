//! Adapter-internal error type and conversion to `caliban_provider::Error`.

use caliban_provider::Error as ProviderError;

/// Errors produced internally by the Google Gemini adapter.
#[derive(thiserror::Error, Debug)]
pub enum GoogleError {
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

    /// The upstream server returned a JSON error object in the SSE body
    /// (HTTP 200 + an `{"error": {...}}` payload in the stream) rather than
    /// as a non-2xx HTTP status. Gemini does this for mid-stream faults
    /// (e.g. a 500 `INTERNAL`, a 503 overload, or a model-process crash) and
    /// occasionally for a context overflow detected after the stream opens.
    /// Carrying the upstream message verbatim lets the `From` conversion
    /// classify it (context-overflow → `ContextTooLong`, server fault →
    /// `UpstreamServerFault`) instead of silently parsing the error object
    /// into an empty response chunk.
    #[error("upstream error: {0}")]
    UpstreamError(String),

    /// A required environment-variable or config field was absent.
    #[error("missing config field: {0}")]
    MissingConfig(&'static str),

    /// A generic transport-level error.
    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),

    /// An invalid or unsupported request was made.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

/// Context-window-overflow markers. Covers Gemini's native phrasing ("input
/// token count (N) exceeds the maximum number of tokens allowed (M)") plus the
/// OpenAI-compatible phrasings a proxy might surface.
const CONTEXT_MARKERS: &[&str] = &[
    "exceeds the maximum number of tokens",
    "input token count",
    "context_length_exceeded",
    "Input tokens exceed",
    "Please reduce the length",
    "context window",
    "maximum context length",
];

/// Server-side fault markers. Includes Gemini's generic server-fault phrasings
/// (`INTERNAL`, overloaded, `UNAVAILABLE`) — safe because this only runs against
/// server-authored error text, never user request content.
const FAULT_MARKERS: &[&str] = &[
    "Internal error",
    "INTERNAL",
    "overloaded",
    "UNAVAILABLE",
    "crashed",
    "Exit code:",
    "out of memory",
    "Out of memory",
    "OOMKilled",
    "segmentation fault",
    "Segmentation fault",
    "killed by signal",
    "Killed by signal",
];

/// Token-count markers: Gemini's parenthesized phrasing first, then the
/// OpenAI-style "limit of … resulted in …" phrasing.
const MAX_TOKEN_MARKERS: &[&str] = &["allowed (", "limit of "];
const REQUESTED_TOKEN_MARKERS: &[&str] = &["token count (", "resulted in "];

/// Classify a server-authored error body (a non-2xx body or an in-band SSE
/// error payload): context overflow → `ContextTooLong` (fires reactive
/// compaction), server-side fault → `UpstreamServerFault`, else `InvalidRequest`
/// verbatim.
fn classify_error_body(body: &str) -> ProviderError {
    use caliban_provider::error_classify as ec;
    ec::classify_context_too_long(
        body,
        CONTEXT_MARKERS,
        MAX_TOKEN_MARKERS,
        REQUESTED_TOKEN_MARKERS,
    )
    .or_else(|| ec::classify_upstream_server_fault(body, FAULT_MARKERS))
    .unwrap_or_else(|| ProviderError::InvalidRequest(body.to_string()))
}

impl GoogleError {
    /// Build [`GoogleError::BadStatus`] from the shared
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

impl From<GoogleError> for ProviderError {
    fn from(e: GoogleError) -> Self {
        use caliban_provider::TransportErrorClass;
        match e {
            GoogleError::Http(ref err) => {
                match caliban_provider::classify_reqwest_error("google", err) {
                    TransportErrorClass::StreamInterrupted => ProviderError::stream_interrupted(
                        caliban_provider::render_source_chain(err),
                    ),
                    TransportErrorClass::Network => ProviderError::network(e),
                    TransportErrorClass::Adapter => ProviderError::adapter(e),
                }
            }
            GoogleError::BadStatus {
                status,
                ref body,
                retry_after,
            } => caliban_provider::error_classify::map_bad_status(
                status,
                body,
                retry_after,
                classify_error_body,
            )
            .unwrap_or_else(|| ProviderError::adapter(e)),
            GoogleError::InvalidRequest(ref msg) => ProviderError::InvalidRequest(msg.clone()),
            // In-band SSE error payload — server-authored text, classified the
            // same way as a non-2xx body.
            GoogleError::UpstreamError(ref msg) => classify_error_body(msg),
            GoogleError::Deserialize(_)
            | GoogleError::StreamParse(_)
            | GoogleError::MissingConfig(_)
            | GoogleError::Transport(_) => ProviderError::adapter(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_400(body: &str) -> ProviderError {
        ProviderError::from(GoogleError::BadStatus {
            status: 400,
            body: body.to_string(),
            retry_after: None,
        })
    }

    fn from_upstream(msg: &str) -> ProviderError {
        ProviderError::from(GoogleError::UpstreamError(msg.to_string()))
    }

    #[test]
    fn gemini_400_context_overflow_routes_to_context_too_long() {
        // The verbatim INVALID_ARGUMENT body Gemini returns when the prompt
        // exceeds the model's context window. Without classification this
        // becomes InvalidRequest and the agent loop's reactive-compaction
        // recovery (which only fires on ContextTooLong) never triggers.
        let body = r#"{"error":{"code":400,"message":"The input token count (1290020) exceeds the maximum number of tokens allowed (1048575).","status":"INVALID_ARGUMENT"}}"#;
        match from_400(body) {
            ProviderError::ContextTooLong {
                max_tokens,
                requested_tokens,
            } => {
                assert_eq!(max_tokens, 1_048_575);
                assert_eq!(requested_tokens, 1_290_020);
            }
            other => panic!("expected ContextTooLong, got {other:?}"),
        }
    }

    #[test]
    fn gemini_400_invalid_argument_stays_invalid_request() {
        // A genuine request-validation 400 must NOT be misclassified.
        let body = r#"{"error":{"code":400,"message":"Invalid value at 'generation_config.temperature' (2.5 out of range)","status":"INVALID_ARGUMENT"}}"#;
        match from_400(body) {
            ProviderError::InvalidRequest(s) => assert!(s.contains("temperature")),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn other_status_arms_unchanged() {
        // Regression: classification only touches 400 / UpstreamError; the
        // existing status mapping for 401/429/404/5xx must be untouched.
        assert!(matches!(
            ProviderError::from(GoogleError::BadStatus {
                status: 401,
                body: "nope".into(),
                retry_after: None,
            }),
            ProviderError::Auth(_)
        ));
        // A 429 carrying a Retry-After hint must surface it on RateLimit.
        assert!(matches!(
            ProviderError::from(GoogleError::BadStatus {
                status: 429,
                body: String::new(),
                retry_after: Some(std::time::Duration::from_secs(12)),
            }),
            ProviderError::RateLimit {
                retry_after: Some(d),
            } if d == std::time::Duration::from_secs(12)
        ));
        assert!(matches!(
            ProviderError::from(GoogleError::BadStatus {
                status: 503,
                body: "overloaded".into(),
                retry_after: None,
            }),
            ProviderError::ServerError { status: 503, .. }
        ));
    }

    #[test]
    fn upstream_in_band_context_overflow_routes_to_context_too_long() {
        // An in-band SSE error payload carrying a context-overflow message
        // must route to ContextTooLong so reactive compaction fires even
        // mid-stream (HTTP 200 + error object in the SSE body).
        let msg =
            "The input token count (5200) exceeds the maximum number of tokens allowed (4096).";
        match from_upstream(msg) {
            ProviderError::ContextTooLong {
                max_tokens,
                requested_tokens,
            } => {
                assert_eq!(max_tokens, 4096);
                assert_eq!(requested_tokens, 5200);
            }
            other => panic!("expected ContextTooLong, got {other:?}"),
        }
    }

    #[test]
    fn upstream_in_band_internal_fault_routes_to_server_fault() {
        // Gemini's canonical 500 INTERNAL in-band message.
        assert!(matches!(
            from_upstream("Internal error encountered."),
            ProviderError::UpstreamServerFault(_)
        ));
    }

    #[test]
    fn upstream_in_band_model_crash_routes_to_server_fault() {
        // The issue's example: a mid-stream model-process crash.
        assert!(matches!(
            from_upstream("model crashed mid-generation"),
            ProviderError::UpstreamServerFault(_)
        ));
    }

    #[test]
    fn upstream_in_band_overloaded_routes_to_server_fault() {
        // Gemini's 503 UNAVAILABLE in-band message.
        assert!(matches!(
            from_upstream("The model is overloaded. Please try again later."),
            ProviderError::UpstreamServerFault(_)
        ));
    }

    #[test]
    fn upstream_in_band_plain_message_stays_invalid_request() {
        // No context / fault markers → request-shaped fallback, message verbatim.
        match from_upstream("function call schema rejected: unknown field 'foo'") {
            ProviderError::InvalidRequest(s) => assert!(s.contains("schema rejected")),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn context_classifier_wins_over_fault_for_overflow_body() {
        // Ordering guard: context detection must precede fault detection so a
        // context-overflow body never degrades to UpstreamServerFault.
        let msg = "context_length_exceeded: Input tokens exceed the limit";
        assert!(matches!(
            from_upstream(msg),
            ProviderError::ContextTooLong { .. }
        ));
    }
}
