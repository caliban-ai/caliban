//! Fatal-for-route classification + sequential fallback driver.

use caliban_provider::Error as ProviderError;

/// `true` if `err` is fatal for the *current route* — i.e. trying the same
/// request again on a different route may succeed. User-content errors (auth,
/// invalid request, content policy) propagate; provider-side faults
/// (5xx, rate-limit after retries, model unavailable, network timeout,
/// context-too-long) advance to the next candidate.
#[must_use]
pub fn is_fatal_for_route(err: &ProviderError) -> bool {
    match err {
        ProviderError::ModelUnavailable(_)
        | ProviderError::RateLimit { .. }
        | ProviderError::ContextTooLong { .. }
        | ProviderError::ServerError { .. }
        | ProviderError::UpstreamServerFault(_)
        | ProviderError::Network(_)
        | ProviderError::StreamInterrupted(_)
        | ProviderError::StreamIdle(_) => true,
        // Adapter errors are opaque — treat as non-fatal so we don't mask
        // configuration bugs by silently retrying.
        ProviderError::Auth(_)
        | ProviderError::InvalidRequest(_)
        | ProviderError::ContentFilter(_)
        | ProviderError::Cancelled
        | ProviderError::Adapter(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn server_error_is_fatal() {
        let e = ProviderError::ServerError {
            status: 503,
            body: "down".into(),
        };
        assert!(is_fatal_for_route(&e));
    }

    #[test]
    fn rate_limit_after_retries_is_fatal() {
        let e = ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(1)),
        };
        assert!(is_fatal_for_route(&e));
    }

    #[test]
    fn network_error_is_fatal() {
        let e = ProviderError::network(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out",
        ));
        assert!(is_fatal_for_route(&e));
    }

    #[test]
    fn auth_is_not_fatal_for_route() {
        let e = ProviderError::Auth("bad key".into());
        assert!(!is_fatal_for_route(&e));
    }

    #[test]
    fn invalid_request_is_not_fatal_for_route() {
        let e = ProviderError::InvalidRequest("schema".into());
        assert!(!is_fatal_for_route(&e));
    }

    #[test]
    fn content_filter_is_not_fatal_for_route() {
        let e = ProviderError::ContentFilter("policy".into());
        assert!(!is_fatal_for_route(&e));
    }

    #[test]
    fn cancelled_is_not_fatal_for_route() {
        assert!(!is_fatal_for_route(&ProviderError::Cancelled));
    }

    #[test]
    fn model_unavailable_is_fatal_for_route() {
        let e = ProviderError::ModelUnavailable("nope".into());
        assert!(is_fatal_for_route(&e));
    }

    #[test]
    fn stream_interrupted_is_fatal_for_route() {
        // Mid-response interruption should advance to the next route, same
        // as a connect-time network error.
        let e = ProviderError::stream_interrupted("connection reset by peer");
        assert!(is_fatal_for_route(&e));
    }
}
