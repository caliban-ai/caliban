//! Shared classification of `reqwest` transport failures.
//!
//! Every HTTP adapter (`anthropic`, `google`, `openai`, `ollama`, …) wraps a
//! `reqwest::Error` in its own `Http` variant and must decide which
//! [`crate::Error`] it maps to. That decision is identical across adapters, so
//! it lives here once:
//!
//! - **body/decode** failures fire *after* the request was accepted and the
//!   response began streaming → [`TransportErrorClass::StreamInterrupted`], so
//!   the user sees "stream interrupted mid-response" instead of the misleading
//!   `network error: HTTP request failed: error decoding response body` chain.
//! - **connect/timeout** failures (no usable response arrived) →
//!   [`TransportErrorClass::Network`].
//! - everything else → [`TransportErrorClass::Adapter`].
//!
//! Adapters call [`classify_reqwest_error`] (which also logs the error shape
//! under the shared `caliban_provider::transport` target) and apply the
//! returned class, wrapping their own adapter error for the `Network`/`Adapter`
//! cases so the `HTTP request failed: …` Display prefix is preserved.
//!
//! It also owns the **non-2xx status check** ([`check_status`]) that every
//! adapter's `send`/`stream` repeats verbatim, and the `Retry-After` parsing
//! ([`parse_retry_after`]) that feeds the 429 backoff hint into
//! [`crate::Error::RateLimit`].

use std::time::Duration;

/// A non-2xx HTTP response captured by [`check_status`] for the caller to turn
/// into its adapter-specific `BadStatus` error variant.
///
/// Carries the `Retry-After` hint (parsed from the response header *before* the
/// body is consumed) so the 429 path can populate
/// [`crate::Error::RateLimit`]'s `retry_after` and the agent-core retry loop can
/// honor the server's requested backoff instead of falling back to exponential
/// backoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response body text (best-effort; empty if it could not be read).
    pub body: String,
    /// The `Retry-After` delay, when the server sent a delta-seconds header.
    pub retry_after: Option<Duration>,
}

/// Parse a `Retry-After` header in its delta-seconds form into a [`Duration`].
///
/// Only the integer-seconds form (`Retry-After: 30`) is honored — the rarer
/// HTTP-date form returns `None`, since 429 responses in practice use
/// delta-seconds and a missing hint simply falls back to the retry policy's
/// exponential backoff. Returns `None` when the header is absent, non-ASCII, or
/// not a bare integer.
#[must_use]
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// Check a response's HTTP status: return it unchanged on 2xx, or consume the
/// body and build an adapter error via `mk_err` on any non-2xx status.
///
/// This replaces the `if !status.is_success() { … BadStatus { … } }` block that
/// had been copy-pasted across every adapter's `send`/`stream`. The
/// `Retry-After` header is captured into [`BadResponse`] *before* the body is
/// read, so adapters get the 429 backoff hint for free.
///
/// # Errors
///
/// Returns `Err(mk_err(BadResponse { … }))` for any non-2xx status.
pub async fn check_status<E>(
    resp: reqwest::Response,
    mk_err: impl FnOnce(BadResponse) -> E,
) -> Result<reqwest::Response, E> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    // Capture the header before `text()` consumes the response.
    let retry_after = parse_retry_after(resp.headers());
    let body = resp.text().await.unwrap_or_default();
    Err(mk_err(BadResponse {
        status: status.as_u16(),
        body,
        retry_after,
    }))
}

/// Which [`crate::Error`] an adapter's `reqwest`-backed `Http` error maps to.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TransportErrorClass {
    /// Body/decode failure: the request was accepted and the response began
    /// streaming before the transport broke. Apply with
    /// [`crate::Error::stream_interrupted`] + [`render_source_chain`].
    StreamInterrupted,
    /// Connect/timeout failure before a usable response arrived. Apply with
    /// [`crate::Error::network`].
    Network,
    /// Anything else (request-build errors, etc.). Apply with
    /// [`crate::Error::adapter`].
    Adapter,
}

/// Pure flag→class mapping. Split out from [`classify_reqwest_error`] so it is
/// unit-testable — `reqwest::Error` has no public constructor, so the decision
/// logic must be exercised on the raw predicates. Body/decode is checked
/// *first* so a body-read that also trips `is_timeout()` (a slow upstream that
/// stalls mid-stream) is still reported as a stream interruption rather than a
/// generic network timeout.
#[allow(clippy::fn_params_excessive_bools)]
#[must_use]
fn classify_flags(
    is_connect: bool,
    is_timeout: bool,
    is_body: bool,
    is_decode: bool,
) -> TransportErrorClass {
    if is_body || is_decode {
        TransportErrorClass::StreamInterrupted
    } else if is_connect || is_timeout {
        TransportErrorClass::Network
    } else {
        TransportErrorClass::Adapter
    }
}

/// Render an error and its source chain as `"top: child: grandchild"`, so the
/// `StreamInterrupted` message and the transport log surface the underlying
/// reason (the `hyper`/`io::Error` that severed the body) instead of just the
/// top-level `reqwest` Display.
#[must_use]
pub fn render_source_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut cursor: Option<&(dyn std::error::Error + 'static)> = err.source();
    while let Some(next) = cursor {
        let msg = next.to_string();
        // reqwest's Display already includes the immediate source in some
        // variants; skip duplicates that would otherwise read "X: X".
        if !out.ends_with(&msg) {
            out.push_str(": ");
            out.push_str(&msg);
        }
        cursor = next.source();
    }
    out
}

/// Classify a `reqwest` transport error and log its shape under the shared
/// `caliban_provider::transport` target (with a `provider` field) so operators
/// can see which failure flavor fired — timeout vs. connect vs. mid-body
/// interruption — without enabling per-crate trace logs.
///
/// Returns the [`TransportErrorClass`]; the caller applies it, wrapping its own
/// adapter error for `Network`/`Adapter` (to keep the `HTTP request failed: …`
/// Display prefix) and using [`render_source_chain`] for `StreamInterrupted`.
#[must_use]
pub fn classify_reqwest_error(provider: &str, err: &reqwest::Error) -> TransportErrorClass {
    let class = classify_flags(
        err.is_connect(),
        err.is_timeout(),
        err.is_body(),
        err.is_decode(),
    );
    tracing::warn!(
        target: "caliban_provider::transport",
        provider,
        is_connect = err.is_connect(),
        is_timeout = err.is_timeout(),
        is_body = err.is_body(),
        is_decode = err.is_decode(),
        is_request = err.is_request(),
        url = err.url().map(reqwest::Url::as_str),
        source_chain = %render_source_chain(err),
        class = ?class,
        "provider transport error"
    );
    class
}

#[cfg(test)]
mod tests {
    use super::{TransportErrorClass, classify_flags, parse_retry_after, render_source_chain};
    use std::time::Duration;

    fn headers_with(name: &'static str, value: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(name, reqwest::header::HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn retry_after_parses_delta_seconds() {
        let h = headers_with("retry-after", "30");
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(30)));
    }

    #[test]
    fn retry_after_tolerates_surrounding_whitespace() {
        let h = headers_with("retry-after", "  5 ");
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(5)));
    }

    #[test]
    fn retry_after_absent_is_none() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn retry_after_http_date_form_is_none() {
        // The HTTP-date form is intentionally not parsed — falls back to backoff.
        let h = headers_with("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT");
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn retry_after_non_numeric_is_none() {
        let h = headers_with("retry-after", "soon");
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn body_error_classifies_as_stream_interrupted() {
        // The reqwest "error decoding response body" case: the request was
        // accepted and the response began streaming before the body broke.
        assert_eq!(
            classify_flags(false, false, true, false),
            TransportErrorClass::StreamInterrupted
        );
    }

    #[test]
    fn decode_error_classifies_as_stream_interrupted() {
        assert_eq!(
            classify_flags(false, false, false, true),
            TransportErrorClass::StreamInterrupted
        );
    }

    #[test]
    fn body_read_timeout_prefers_stream_interrupted_over_network() {
        // A slow upstream that stalls mid-body trips both is_body() and
        // is_timeout(); body/decode must win so the user sees "stream
        // interrupted" rather than a generic network timeout. This is the
        // exact shape behind the production "network error: HTTP request
        // failed: error decoding response body" report.
        assert_eq!(
            classify_flags(false, true, true, false),
            TransportErrorClass::StreamInterrupted
        );
    }

    #[test]
    fn connect_error_classifies_as_network() {
        assert_eq!(
            classify_flags(true, false, false, false),
            TransportErrorClass::Network
        );
    }

    #[test]
    fn timeout_without_body_classifies_as_network() {
        assert_eq!(
            classify_flags(false, true, false, false),
            TransportErrorClass::Network
        );
    }

    #[test]
    fn other_http_error_classifies_as_adapter() {
        assert_eq!(
            classify_flags(false, false, false, false),
            TransportErrorClass::Adapter
        );
    }

    #[test]
    fn source_chain_joins_nested_causes() {
        use std::error::Error;
        use std::fmt;

        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "connection reset by peer")
            }
        }
        impl Error for Inner {}

        #[derive(Debug)]
        struct Outer;
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "error decoding response body")
            }
        }
        impl Error for Outer {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&Inner)
            }
        }

        assert_eq!(
            render_source_chain(&Outer),
            "error decoding response body: connection reset by peer"
        );
    }
}
