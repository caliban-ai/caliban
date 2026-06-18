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

    /// The configured base URL (typically from `OPENAI_BASE_URL`) is not
    /// a parseable URL. Distinct from `Transport` so the binary's
    /// startup dispatch can surface a config-shaped error message
    /// instead of collapsing into "API key is missing" (lmstudio probe
    /// 2026-05-27 Finding 2).
    #[error("invalid base URL {value:?}: {source}")]
    InvalidBaseUrl {
        /// The unparseable value that the operator supplied.
        value: String,
        /// The underlying `url::ParseError`.
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// An unsupported feature was requested.
    #[error("unsupported feature: {0}")]
    Unsupported(String),
}

/// Context-window-overflow markers in OpenAI-compatible error bodies. Both
/// HTTP 400 bodies and LM Studio's in-band SSE error bodies use these.
const CONTEXT_MARKERS: &[&str] = &[
    "context_length_exceeded",
    "Input tokens exceed",
    "Please reduce the length",
    "context window",
    "maximum context length",
];

/// Server-side fault markers (model-process crash, OOM kill, segfault, …).
/// Conservative + case-sensitive: only well-anchored phrases unlikely to
/// appear in a legitimate request-validation message. In particular the crash
/// marker is the full "crashed without additional information" envelope, not a
/// bare "crashed", so an ordinary error mentioning "crashed" stays
/// `InvalidRequest`.
const FAULT_MARKERS: &[&str] = &[
    "crashed without additional information",
    "Exit code:",
    "out of memory",
    "Out of memory",
    "OOMKilled",
    "segmentation fault",
    "Segmentation fault",
    "killed by signal",
    "Killed by signal",
];

/// Token-count markers for the OpenAI-style "limit of 272000 … resulted in
/// 281292" phrasing.
const MAX_TOKEN_MARKERS: &[&str] = &["limit of "];
const REQUESTED_TOKEN_MARKERS: &[&str] = &["resulted in "];

/// Classify a server-authored error body (an HTTP 400 body or an in-band SSE
/// error payload): a context-window overflow → `ContextTooLong` (so the agent
/// loop's reactive-compaction recovery fires), a server-side fault →
/// `UpstreamServerFault` (so the surface line attributes the fault to the
/// server), else the body verbatim as `InvalidRequest`.
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

impl From<OpenAIError> for ProviderError {
    fn from(e: OpenAIError) -> Self {
        use caliban_provider::TransportErrorClass;
        match e {
            OpenAIError::Http(ref err) => {
                match caliban_provider::classify_reqwest_error("openai", err) {
                    TransportErrorClass::StreamInterrupted => ProviderError::stream_interrupted(
                        caliban_provider::render_source_chain(err),
                    ),
                    TransportErrorClass::Network => ProviderError::network(e),
                    TransportErrorClass::Adapter => ProviderError::adapter(e),
                }
            }
            OpenAIError::BadStatus { status, ref body } => {
                caliban_provider::error_classify::map_bad_status(status, body, classify_error_body)
                    .unwrap_or_else(|| ProviderError::adapter(e))
            }
            OpenAIError::Deserialize(_)
            | OpenAIError::StreamParse(_)
            | OpenAIError::MissingConfig(_)
            | OpenAIError::Transport(_)
            | OpenAIError::InvalidBaseUrl { .. }
            | OpenAIError::Unsupported(_) => ProviderError::adapter(e),
            // In-band SSE error payload — server-authored text, classified the
            // same way as a non-2xx body.
            OpenAIError::UpstreamError(ref msg) => classify_error_body(msg),
        }
    }
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
        //   docs/2026-05-27-lmstudio-probe-findings.md — context_length_exceeded
        //   against a 272K-token model
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
        // "crashed" alone (no anchor markers) must not trip the server-fault
        // classifier — it stays InvalidRequest.
        let e = OpenAIError::UpstreamError("model loaded but inference engine crashed".into());
        match ProviderError::from(e) {
            ProviderError::InvalidRequest(s) => assert!(s.contains("inference")),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn upstream_in_band_lmstudio_crash_envelope_routes_to_server_fault() {
        // Finding 9 (lmstudio probe 2026-05-27): LM Studio returns HTTP 200
        // with `{"error":{"message":"The model has crashed without additional
        // information. (Exit code: null)"}}` when the model process exits
        // mid-stream. The user-visible message must read "upstream server
        // fault: …" so the operator understands the fault is server-side,
        // not request-side.
        let body =
            "The model has crashed without additional information. (Exit code: null)".to_string();
        let e = OpenAIError::UpstreamError(body.clone());
        match ProviderError::from(e) {
            ProviderError::UpstreamServerFault(s) => {
                assert_eq!(s, body);
                // Spot-check the surface-line shape — the operator sees this.
                assert_eq!(
                    ProviderError::UpstreamServerFault(s).to_string(),
                    format!("upstream server fault: {body}")
                );
            }
            other => panic!("expected UpstreamServerFault, got {other:?}"),
        }
    }

    #[test]
    fn upstream_in_band_oom_routes_to_server_fault() {
        let body = "model worker out of memory (oom kill).";
        let e = OpenAIError::UpstreamError(body.into());
        assert!(matches!(
            ProviderError::from(e),
            ProviderError::UpstreamServerFault(_)
        ));
    }

    #[test]
    fn upstream_in_band_segfault_routes_to_server_fault() {
        let body = "worker died: Segmentation fault (core dumped)";
        let e = OpenAIError::UpstreamError(body.into());
        assert!(matches!(
            ProviderError::from(e),
            ProviderError::UpstreamServerFault(_)
        ));
    }

    #[test]
    fn upstream_in_band_killed_by_signal_routes_to_server_fault() {
        let body = "child process killed by signal 9 (SIGKILL)";
        let e = OpenAIError::UpstreamError(body.into());
        assert!(matches!(
            ProviderError::from(e),
            ProviderError::UpstreamServerFault(_)
        ));
    }

    #[test]
    fn upstream_in_band_context_overflow_still_routes_to_context_too_long() {
        // Server-fault classifier must not preempt context-too-long
        // detection (which fires the agent loop's reactive compaction).
        let body = "Input tokens exceed the configured limit of 4096 tokens. Your messages resulted in 5200 tokens. Please reduce the length of the messages.";
        let e = OpenAIError::UpstreamError(body.to_string());
        assert!(matches!(
            ProviderError::from(e),
            ProviderError::ContextTooLong { .. }
        ));
    }
}
