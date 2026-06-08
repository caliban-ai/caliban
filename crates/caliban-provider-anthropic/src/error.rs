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
            AnthropicError::BadStatus { status, ref body } => match status {
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
            AnthropicError::Deserialize(_)
            | AnthropicError::StreamParse(_)
            | AnthropicError::MissingConfig(_)
            | AnthropicError::Transport(_) => ProviderError::adapter(e),
        }
    }
}

/// Inspect a 400 error body for context-window-exceeded patterns and
/// route to `ProviderError::ContextTooLong` so the agent loop's
/// reactive-compaction recovery can fire. Anthropic's shape is
/// `{"error":{"type":"invalid_request_error","message":"prompt is too long: X tokens > Y maximum"}}`.
fn classify_context_length_exceeded(body: &str) -> Option<ProviderError> {
    let is_context_error = body.contains("prompt is too long")
        || body.contains("context window")
        || body.contains("maximum context length")
        || body.contains("context_length_exceeded");
    if !is_context_error {
        return None;
    }
    let (requested_tokens, max_tokens) = parse_anthropic_token_counts(body);
    Some(ProviderError::ContextTooLong {
        max_tokens,
        requested_tokens,
    })
}

/// Best-effort extraction of (`requested_tokens`, `max_tokens`) from an
/// Anthropic-style "prompt is too long: 210000 tokens > 200000 maximum" body.
fn parse_anthropic_token_counts(body: &str) -> (u32, u32) {
    let req = extract_u32_after(body, "too long: ").unwrap_or(0);
    let max = extract_u32_after(body, "> ").unwrap_or(0);
    (req, max)
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

    #[test]
    fn classifies_prompt_too_long_extracts_token_counts() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#;
        let e = AnthropicError::BadStatus {
            status: 400,
            body: body.to_string(),
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
        };
        match ProviderError::from(e) {
            ProviderError::InvalidRequest(s) => assert!(s.contains("invalid messages")),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }
}
