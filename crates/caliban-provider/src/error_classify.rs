//! Shared HTTP-error-body classification for provider adapters.
//!
//! Every HTTP adapter (anthropic, openai, google, ollama) maps a non-2xx
//! `BadStatus { status, body }` — and an in-band SSE/`UpstreamError` body — onto
//! [`crate::Error`]. The status arms (401/403 → auth, 429 → rate limit, 404 →
//! model-unavailable, 5xx → server error) and the body-sniffing helpers
//! (context-window-overflow detection, upstream-fault detection, token-count
//! extraction) had been copy-pasted across the adapters and had begun to drift
//! — e.g. the `OpenAI` 400 path ran only the context classifier while
//! `Google`'s ran both. This module owns the *mechanism* once; each adapter
//! passes its own
//! provider-specific marker sets (the *data*, which legitimately differs), so a
//! new crash marker or context phrasing is added in one place per provider and
//! the recovery-driving classification can no longer silently diverge.

use crate::Error as ProviderError;
use std::time::Duration;

/// Map a non-2xx HTTP status onto the provider-agnostic [`ProviderError`]
/// arms shared by every adapter, returning `None` for statuses with no shared
/// mapping so the caller can apply its own adapter-specific fallback (which
/// typically wraps the original adapter error via `ProviderError::adapter`).
///
/// `retry_after` is the parsed `Retry-After` hint
/// ([`crate::transport::parse_retry_after`]); it populates the 429
/// [`ProviderError::RateLimit`] so the agent-core retry loop honors the
/// server's requested backoff. Pass `None` when no hint was present.
///
/// `on_400` builds the error for a `400` body — adapters differ here (some
/// classify context-overflow / upstream-fault, some return a plain
/// `InvalidRequest`). Returning `Option` (rather than taking an `on_other`
/// closure) keeps the original adapter error borrowable: the returned value
/// does not borrow `body`, so the caller can move the error into its
/// `unwrap_or_else(|| ProviderError::adapter(e))` fallback.
pub fn map_bad_status(
    status: u16,
    body: &str,
    retry_after: Option<Duration>,
    on_400: impl FnOnce(&str) -> ProviderError,
) -> Option<ProviderError> {
    match status {
        401 | 403 => Some(ProviderError::Auth(body.to_string())),
        429 => Some(ProviderError::RateLimit { retry_after }),
        400 => Some(on_400(body)),
        404 => Some(ProviderError::ModelUnavailable(body.to_string())),
        _ if status >= 500 => Some(ProviderError::ServerError {
            status,
            body: body.to_string(),
        }),
        _ => None,
    }
}

/// When `body` contains any of `context_markers`, classify it as
/// [`ProviderError::ContextTooLong`] (so the agent loop's reactive-compaction
/// recovery fires), extracting `(max_tokens, requested_tokens)` best-effort via
/// [`parse_token_counts`]. Returns `None` when no context marker matches.
#[must_use]
pub fn classify_context_too_long(
    body: &str,
    context_markers: &[&str],
    max_token_markers: &[&str],
    requested_token_markers: &[&str],
) -> Option<ProviderError> {
    if !context_markers.iter().any(|m| body.contains(m)) {
        return None;
    }
    let (max_tokens, requested_tokens) =
        parse_token_counts(body, max_token_markers, requested_token_markers);
    Some(ProviderError::ContextTooLong {
        max_tokens,
        requested_tokens,
    })
}

/// When `body` contains any of `fault_markers`, classify it as
/// [`ProviderError::UpstreamServerFault`] (so the user-visible line attributes
/// the failure to the server, not the request). Returns `None` otherwise.
///
/// Only ever run against server-authored error text (a non-2xx body or an
/// in-band `error` object), never user request content.
#[must_use]
pub fn classify_upstream_server_fault(body: &str, fault_markers: &[&str]) -> Option<ProviderError> {
    if !fault_markers.iter().any(|m| body.contains(m)) {
        return None;
    }
    Some(ProviderError::UpstreamServerFault(body.to_string()))
}

/// Best-effort `(max_tokens, requested_tokens)` extraction: the first
/// `max_token_markers` entry that yields a number wins for the max, likewise
/// for the requested count. Missing markers default to `0` (the agent loop
/// branches only on the [`ProviderError::ContextTooLong`] variant, not the
/// numbers).
#[must_use]
pub fn parse_token_counts(
    body: &str,
    max_token_markers: &[&str],
    requested_token_markers: &[&str],
) -> (u32, u32) {
    let max = max_token_markers
        .iter()
        .find_map(|m| extract_u32_after(body, m))
        .unwrap_or(0);
    let requested = requested_token_markers
        .iter()
        .find_map(|m| extract_u32_after(body, m))
        .unwrap_or(0);
    (max, requested)
}

/// Parse the run of ASCII digits immediately following the first occurrence of
/// `marker` in `body`. Returns `None` when the marker is absent or not followed
/// by digits.
#[must_use]
pub fn extract_u32_after(body: &str, marker: &str) -> Option<u32> {
    let idx = body.find(marker)?;
    let after = &body[idx + marker.len()..];
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_bad_status_shared_arms() {
        let mk400 = |_: &str| ProviderError::InvalidRequest("x".into());
        assert!(matches!(
            map_bad_status(401, "no", None, mk400),
            Some(ProviderError::Auth(_))
        ));
        assert!(matches!(
            map_bad_status(403, "no", None, mk400),
            Some(ProviderError::Auth(_))
        ));
        assert!(matches!(
            map_bad_status(429, "", None, mk400),
            Some(ProviderError::RateLimit { retry_after: None })
        ));
        assert!(matches!(
            map_bad_status(404, "gone", None, mk400),
            Some(ProviderError::ModelUnavailable(_))
        ));
        assert!(matches!(
            map_bad_status(503, "down", None, mk400),
            Some(ProviderError::ServerError { status: 503, .. })
        ));
        // Unmapped status → None so the caller applies its own fallback.
        assert!(map_bad_status(418, "teapot", None, mk400).is_none());
    }

    #[test]
    fn map_bad_status_429_carries_retry_after() {
        // The Retry-After hint must flow into RateLimit so the retry loop can
        // honor the server's requested backoff.
        let mk400 = |_: &str| ProviderError::InvalidRequest("x".into());
        assert!(matches!(
            map_bad_status(429, "", Some(Duration::from_secs(7)), mk400),
            Some(ProviderError::RateLimit {
                retry_after: Some(d),
            }) if d == Duration::from_secs(7)
        ));
    }

    #[test]
    fn map_bad_status_400_delegates_to_on_400() {
        let got = map_bad_status(400, "too big", None, |b| {
            classify_context_too_long(b, &["too big"], &[], &[])
                .unwrap_or_else(|| ProviderError::InvalidRequest(b.to_string()))
        });
        assert!(matches!(got, Some(ProviderError::ContextTooLong { .. })));
    }

    #[test]
    fn context_classifier_extracts_counts() {
        let body = "Input tokens exceed the configured limit of 272000 tokens. \
                    Your messages resulted in 281292 tokens";
        let got = classify_context_too_long(
            body,
            &["Input tokens exceed"],
            &["limit of "],
            &["resulted in "],
        );
        match got {
            Some(ProviderError::ContextTooLong {
                max_tokens,
                requested_tokens,
            }) => {
                assert_eq!(max_tokens, 272_000);
                assert_eq!(requested_tokens, 281_292);
            }
            other => panic!("expected ContextTooLong, got {other:?}"),
        }
    }

    #[test]
    fn context_classifier_returns_none_without_marker() {
        assert!(classify_context_too_long("totally fine", &["nope"], &[], &[]).is_none());
    }

    #[test]
    fn fault_classifier_matches_markers() {
        assert!(matches!(
            classify_upstream_server_fault("model crashed (Exit code: null)", &["Exit code:"]),
            Some(ProviderError::UpstreamServerFault(_))
        ));
        assert!(classify_upstream_server_fault("bad input", &["Exit code:"]).is_none());
    }

    #[test]
    fn extract_u32_after_reads_digit_run() {
        assert_eq!(
            extract_u32_after("limit of 272000 tokens", "limit of "),
            Some(272_000)
        );
        assert_eq!(extract_u32_after("no number here", "of "), None);
        assert_eq!(extract_u32_after("missing marker", "limit of "), None);
    }

    #[test]
    fn parse_token_counts_tries_markers_in_order() {
        // Gemini parenthesized phrasing via the second marker in each slice.
        let body =
            "input token count (281292) exceeds the maximum number of tokens allowed (272000)";
        let (max, req) = parse_token_counts(
            body,
            &["allowed (", "limit of "],
            &["token count (", "resulted in "],
        );
        assert_eq!(max, 272_000);
        assert_eq!(req, 281_292);
    }
}
