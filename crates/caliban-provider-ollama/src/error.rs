//! Adapter-internal error type and conversion to `caliban_provider::Error`.

use caliban_provider::Error as ProviderError;

/// Errors produced internally by the Ollama adapter.
#[derive(thiserror::Error, Debug)]
pub enum OllamaError {
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

    /// NDJSON stream parse failed.
    #[error("stream parse error: {0}")]
    StreamParse(String),

    /// A required environment-variable or config field was absent.
    #[error("missing config field: {0}")]
    MissingConfig(&'static str),

    /// A generic transport-level error.
    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),

    /// An unsupported feature was requested.
    #[error("unsupported feature: {0}")]
    Unsupported(String),
}

/// How a `reqwest` transport error should be classified into a
/// `ProviderError`. Factored out of the `From` impl so the flag→variant
/// mapping is unit-testable — `reqwest::Error` has no public constructor, so
/// we cannot fabricate one in a test and must test the decision logic on the
/// raw flags instead. Mirrors the `OpenAI` adapter's `Http` arm.
#[derive(Debug, PartialEq, Eq)]
enum HttpErrorClass {
    /// Body/decode failure: the request was accepted and the response began
    /// streaming before the transport broke. Surfaced as `StreamInterrupted`
    /// so the user sees "stream interrupted mid-response" instead of the
    /// misleading `network error: HTTP request failed: error decoding
    /// response body` chain.
    StreamInterrupted,
    /// Connect/timeout failure before a usable response arrived.
    Network,
    /// Anything else (request-build errors, etc.).
    Adapter,
}

/// Map `reqwest::Error` predicates to a [`HttpErrorClass`]. Body/decode is
/// checked *first* so a body-read that also trips `is_timeout()` (e.g. a slow
/// upstream that stalls mid-stream) is still reported as a stream interruption
/// rather than a generic network timeout.
// The four params mirror `reqwest::Error`'s predicate methods 1:1; collapsing
// them into enums (clippy's suggestion) would obscure that direct mapping.
#[allow(clippy::fn_params_excessive_bools)]
fn classify_http_error(
    is_connect: bool,
    is_timeout: bool,
    is_body: bool,
    is_decode: bool,
) -> HttpErrorClass {
    if is_body || is_decode {
        HttpErrorClass::StreamInterrupted
    } else if is_connect || is_timeout {
        HttpErrorClass::Network
    } else {
        HttpErrorClass::Adapter
    }
}

/// Render an error and its source chain as `"top: child: grandchild"`, so the
/// `StreamInterrupted` message and the transport log surface the underlying
/// reason (the `hyper`/`io::Error` that severed the body) instead of just the
/// top-level `reqwest` Display. Ported from the `OpenAI` adapter.
fn render_source_chain(err: &(dyn std::error::Error + 'static)) -> String {
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

impl From<OllamaError> for ProviderError {
    fn from(e: OllamaError) -> Self {
        match e {
            OllamaError::Http(ref err) => {
                // Always log the reqwest error shape — operators have asked
                // for visibility into which transport-failure flavor fired
                // (timeout vs. connect vs. mid-body interruption) without
                // needing to enable per-crate trace logs.
                tracing::warn!(
                    target: "caliban_provider_ollama::transport",
                    is_connect = err.is_connect(),
                    is_timeout = err.is_timeout(),
                    is_body = err.is_body(),
                    is_decode = err.is_decode(),
                    is_request = err.is_request(),
                    url = err.url().map(reqwest::Url::as_str),
                    source_chain = %render_source_chain(err),
                    "ollama transport error"
                );
                match classify_http_error(
                    err.is_connect(),
                    err.is_timeout(),
                    err.is_body(),
                    err.is_decode(),
                ) {
                    HttpErrorClass::StreamInterrupted => {
                        ProviderError::stream_interrupted(render_source_chain(err))
                    }
                    HttpErrorClass::Network => ProviderError::network(e),
                    HttpErrorClass::Adapter => ProviderError::adapter(e),
                }
            }
            OllamaError::BadStatus { status, ref body } => match status {
                401 | 403 => ProviderError::Auth(body.clone()),
                429 => ProviderError::RateLimit { retry_after: None },
                400 => ProviderError::InvalidRequest(body.clone()),
                404 => ProviderError::ModelUnavailable(body.clone()),
                _ if status >= 500 => ProviderError::ServerError {
                    status,
                    body: body.clone(),
                },
                _ => ProviderError::adapter(e),
            },
            OllamaError::Deserialize(_)
            | OllamaError::StreamParse(_)
            | OllamaError::MissingConfig(_)
            | OllamaError::Transport(_)
            | OllamaError::Unsupported(_) => ProviderError::adapter(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_http_error, render_source_chain, HttpErrorClass};

    #[test]
    fn body_error_classifies_as_stream_interrupted() {
        // The reqwest "error decoding response body" case: the request was
        // accepted and the response began streaming before the body broke.
        assert_eq!(
            classify_http_error(false, false, true, false),
            HttpErrorClass::StreamInterrupted
        );
    }

    #[test]
    fn decode_error_classifies_as_stream_interrupted() {
        assert_eq!(
            classify_http_error(false, false, false, true),
            HttpErrorClass::StreamInterrupted
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
            classify_http_error(false, true, true, false),
            HttpErrorClass::StreamInterrupted
        );
    }

    #[test]
    fn connect_error_classifies_as_network() {
        assert_eq!(
            classify_http_error(true, false, false, false),
            HttpErrorClass::Network
        );
    }

    #[test]
    fn timeout_without_body_classifies_as_network() {
        assert_eq!(
            classify_http_error(false, true, false, false),
            HttpErrorClass::Network
        );
    }

    #[test]
    fn other_http_error_classifies_as_adapter() {
        assert_eq!(
            classify_http_error(false, false, false, false),
            HttpErrorClass::Adapter
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
