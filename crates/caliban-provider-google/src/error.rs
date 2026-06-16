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
            GoogleError::BadStatus { status, ref body } => match status {
                401 | 403 => ProviderError::Auth(body.clone()),
                429 => ProviderError::RateLimit { retry_after: None },
                // A Gemini 400 is normally request-shaped, but a context
                // overflow also arrives as a 400 (INVALID_ARGUMENT) — route it
                // to ContextTooLong so the agent loop's reactive-compaction
                // recovery fires. Fall through to the server-fault classifier
                // before the InvalidRequest default (order matches OpenAI).
                400 => classify_context_length_exceeded(body)
                    .or_else(|| classify_upstream_server_fault(body))
                    .unwrap_or_else(|| ProviderError::InvalidRequest(body.clone())),
                404 => ProviderError::ModelUnavailable(body.clone()),
                _ if status >= 500 => ProviderError::ServerError {
                    status,
                    body: body.clone(),
                },
                _ => ProviderError::adapter(e),
            },
            GoogleError::InvalidRequest(ref msg) => ProviderError::InvalidRequest(msg.clone()),
            // In-band SSE error payload. The body is always server-authored
            // error text (never user request content), so classify it the
            // same way: context overflow → ContextTooLong (fires reactive
            // compaction), server-side fault → UpstreamServerFault (attributes
            // the fault to the server), else InvalidRequest verbatim.
            GoogleError::UpstreamError(ref msg) => classify_context_length_exceeded(msg)
                .or_else(|| classify_upstream_server_fault(msg))
                .unwrap_or_else(|| ProviderError::InvalidRequest(msg.clone())),
            GoogleError::Deserialize(_)
            | GoogleError::StreamParse(_)
            | GoogleError::MissingConfig(_)
            | GoogleError::Transport(_) => ProviderError::adapter(e),
        }
    }
}

/// Inspect a 400 / in-band error body for context-window-exceeded patterns.
/// Returns `Some(ContextTooLong { … })` when recognized so the agent loop's
/// reactive-compaction recovery can fire. Covers both Gemini's native phrasing
/// ("input token count (N) exceeds the maximum number of tokens allowed (M)")
/// and the OpenAI-compatible phrasings a proxy might surface. Token counts are
/// extracted best-effort (zero if not parseable); the agent loop only branches
/// on the variant.
fn classify_context_length_exceeded(body: &str) -> Option<ProviderError> {
    let is_context_error = body.contains("exceeds the maximum number of tokens")
        || body.contains("input token count")
        || body.contains("context_length_exceeded")
        || body.contains("Input tokens exceed")
        || body.contains("Please reduce the length")
        || body.contains("context window")
        || body.contains("maximum context length");
    if !is_context_error {
        return None;
    }
    let (max_tokens, requested_tokens) = parse_token_counts(body);
    Some(ProviderError::ContextTooLong {
        max_tokens,
        requested_tokens,
    })
}

/// Inspect a server-authored error body for server-side fault patterns (model
/// process crash, OOM kill, segfault, 500 `INTERNAL`, 503 overload). When
/// matched, returns `Some(UpstreamServerFault { … })` so the user-visible error
/// reads "upstream server fault: <msg>" rather than "invalid request: <msg>" —
/// the failure is server-side, not request-side.
///
/// This only ever runs against error text the server produced (a non-2xx body
/// or an in-band `error` object), never against user request content, so the
/// marker set can include Gemini's generic server-fault phrasings without the
/// false-positive risk a request-content matcher would carry.
fn classify_upstream_server_fault(body: &str) -> Option<ProviderError> {
    let is_fault = body.contains("Internal error")
        || body.contains("INTERNAL")
        || body.contains("overloaded")
        || body.contains("UNAVAILABLE")
        || body.contains("crashed")
        || body.contains("Exit code:")
        || body.contains("out of memory")
        || body.contains("Out of memory")
        || body.contains("OOMKilled")
        || body.contains("segmentation fault")
        || body.contains("Segmentation fault")
        || body.contains("killed by signal")
        || body.contains("Killed by signal");
    if !is_fault {
        return None;
    }
    Some(ProviderError::UpstreamServerFault(body.to_string()))
}

/// Best-effort extraction of (`max_tokens`, `requested_tokens`) from either
/// Gemini's parenthesized phrasing ("input token count (281292) exceeds the
/// maximum number of tokens allowed (272000)") or the OpenAI-style "limit of
/// 272000 … resulted in 281292" phrasing. Missing markers default to 0.
fn parse_token_counts(body: &str) -> (u32, u32) {
    let max = extract_u32_after(body, "allowed (")
        .or_else(|| extract_u32_after(body, "limit of "))
        .unwrap_or(0);
    let req = extract_u32_after(body, "token count (")
        .or_else(|| extract_u32_after(body, "resulted in "))
        .unwrap_or(0);
    (max, req)
}

fn extract_u32_after(body: &str, marker: &str) -> Option<u32> {
    let idx = body.find(marker)?;
    let after = &body[idx + marker.len()..];
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_400(body: &str) -> ProviderError {
        ProviderError::from(GoogleError::BadStatus {
            status: 400,
            body: body.to_string(),
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
                body: "nope".into()
            }),
            ProviderError::Auth(_)
        ));
        assert!(matches!(
            ProviderError::from(GoogleError::BadStatus {
                status: 429,
                body: String::new()
            }),
            ProviderError::RateLimit { .. }
        ));
        assert!(matches!(
            ProviderError::from(GoogleError::BadStatus {
                status: 503,
                body: "overloaded".into()
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
