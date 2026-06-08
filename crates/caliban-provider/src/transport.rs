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
    use super::{TransportErrorClass, classify_flags, render_source_chain};

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
