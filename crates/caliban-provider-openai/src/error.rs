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
                400 => classify_context_length_exceeded(body)
                    .unwrap_or_else(|| ProviderError::InvalidRequest(body.clone())),
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
            // text verbatim — unless the body looks like a context-window
            // overflow, in which case route to ContextTooLong so the agent
            // loop's reactive-compaction recovery fires.
            OpenAIError::UpstreamError(ref msg) => classify_context_length_exceeded(msg)
                .unwrap_or_else(|| ProviderError::InvalidRequest(msg.clone())),
        }
    }
}

/// Inspect a 400/upstream error body for context-window-exceeded patterns.
/// Returns `Some(ContextTooLong { … })` when recognized so the agent loop's
/// reactive-compaction recovery can fire. Both OpenAI-compatible HTTP 400
/// bodies and LM Studio's in-band SSE error bodies use the same patterns.
/// Token counts are extracted best-effort (zero if not parseable); the
/// agent loop only branches on the variant.
fn classify_context_length_exceeded(body: &str) -> Option<ProviderError> {
    let is_context_error = body.contains("context_length_exceeded")
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

/// Best-effort extraction of (`max_tokens`, `requested_tokens`) from an
/// OpenAI-style "Input tokens exceed the configured limit of 272000
/// tokens. Your messages resulted in 281292 tokens" message.
fn parse_token_counts(body: &str) -> (u32, u32) {
    let max = extract_u32_after(body, "limit of ").unwrap_or(0);
    let req = extract_u32_after(body, "resulted in ").unwrap_or(0);
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

    fn assert_context_too_long(body: &str, expected_max: u32, expected_req: u32) {
        let e = OpenAIError::BadStatus {
            status: 400,
            body: body.to_string(),
        };
        match ProviderError::from(e) {
            ProviderError::ContextTooLong {
                max_tokens,
                requested_tokens,
            } => {
                assert_eq!(max_tokens, expected_max);
                assert_eq!(requested_tokens, expected_req);
            }
            other => panic!("expected ContextTooLong, got {other:?}"),
        }
    }

    #[test]
    fn classifies_lmstudio_input_tokens_exceed_message() {
        // The exact body shape observed in production:
        //   docs/TODO.md — context_length_exceeded against a 272K-token model
        let body = r#"{"error":{"message":"Input tokens exceed the configured limit of 272000 tokens. Your messages resulted in 281292 tokens. Please reduce the length of the messages.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#;
        assert_context_too_long(body, 272_000, 281_292);
    }

    #[test]
    fn classifies_openai_code_field_alone() {
        let body = r#"{"error":{"message":"too long","code":"context_length_exceeded"}}"#;
        // No "limit of" / "resulted in" markers — token counts default to 0.
        assert_context_too_long(body, 0, 0);
    }

    #[test]
    fn classifies_context_window_phrasing() {
        let body = r#"{"error":{"message":"This model's maximum context length is 200000 tokens, however you requested 250000 tokens"}}"#;
        // The phrase "maximum context length" matches; the parser doesn't
        // find "limit of " / "resulted in " markers, so counts are 0.
        assert_context_too_long(body, 0, 0);
    }

    #[test]
    fn non_context_400_stays_invalid_request() {
        let body = r#"{"error":{"message":"invalid 'temperature': 9.5 is out of range","code":"invalid_request_error"}}"#;
        let e = OpenAIError::BadStatus {
            status: 400,
            body: body.to_string(),
        };
        match ProviderError::from(e) {
            ProviderError::InvalidRequest(s) => assert!(s.contains("temperature")),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn upstream_in_band_context_error_routes_to_context_too_long() {
        // LM Studio surfaces this same error in-band (HTTP 200 + SSE body
        // with an error object) — Finding 12 in the lmstudio probe.
        let body = "Input tokens exceed the configured limit of 4096 tokens. Your messages resulted in 5200 tokens. Please reduce the length of the messages.";
        let e = OpenAIError::UpstreamError(body.to_string());
        match ProviderError::from(e) {
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
    fn upstream_in_band_non_context_error_stays_invalid_request() {
        let e = OpenAIError::UpstreamError("model loaded but inference engine crashed".into());
        match ProviderError::from(e) {
            ProviderError::InvalidRequest(s) => assert!(s.contains("inference")),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }
}
