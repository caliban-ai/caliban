//! Shared [`reqwest::Client`] factory.
//!
//! Centralizes the boilerplate that had drifted across eight provider
//! transports plus the `web_fetch` / `web_search` tools: user-agent, redirect
//! policy, HTTP/2 preference, default timeout, DNS resolver, and TLS backend.
//!
//! Callers who need provider-specific overrides (custom timeouts, mTLS, proxy
//! configuration) use [`default_client_builder`] and chain extra config onto
//! the returned [`reqwest::ClientBuilder`] before calling `.build()`.
//!
//! The [`no_redirect_client`] variant disables automatic redirect following so
//! the caller can implement its own redirect policy (used by `web_fetch` to
//! enforce same-host redirects manually).

use std::time::Duration;

/// User-agent string applied to every client built by this module.
///
/// Format: `caliban/<CARGO_PKG_VERSION> (+https://github.com/caliban-ai/caliban)`.
pub const USER_AGENT: &str = concat!(
    "caliban/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/caliban-ai/caliban)",
);

/// Default per-request timeout applied to every client built by this module.
///
/// Provider transports that need a different timeout layer it on via
/// [`default_client_builder`] + `.timeout(...)`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Build a [`reqwest::ClientBuilder`] pre-configured with the shared defaults.
///
/// The returned builder has:
/// - `User-Agent: caliban/<version> (+https://github.com/caliban-ai/caliban)`
/// - Redirect follow ≤ 10 (the `reqwest` default — limit kept explicit)
/// - HTTP/2 preferred (negotiated via ALPN)
/// - 30-second per-request timeout
/// - reqwest's default (system / `getaddrinfo`) DNS resolver — the optional
///   `hickory-dns` resolver was dropped in #258 because hickory-proto carried
///   unfixable denial-of-service advisories (RUSTSEC-2026-0118 has no patched
///   release) reachable through this client; the system resolver has no such issue
/// - `rustls` TLS backend (the workspace `reqwest` is built with
///   `default-features = false` + `rustls-tls`)
///
/// Callers layer on provider-specific config (custom headers, longer
/// timeouts, proxies, mTLS roots) and call `.build()` themselves.
pub fn default_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(DEFAULT_TIMEOUT)
        .http2_adaptive_window(true)
}

/// Build a [`reqwest::Client`] with the shared defaults.
///
/// Equivalent to `default_client_builder().build().expect(...)`. Suitable
/// for callers that don't need to layer on additional configuration.
///
/// # Panics
///
/// Panics if the underlying TLS / DNS init fails — which would also panic
/// any provider that tried to build a client and indicates a broken
/// environment, not a configuration error.
#[must_use]
pub fn default_client() -> reqwest::Client {
    default_client_builder()
        .build()
        .expect("default reqwest::Client builds")
}

/// Build a [`reqwest::Client`] with the shared defaults and a caller-supplied
/// per-request timeout.
///
/// Equivalent to `default_client_builder().timeout(timeout).build()`. Provider
/// transports use this instead of repeating the
/// builder → `.timeout(...)` → `.build()` → `.map_err(...)` dance in every
/// adapter `new()`; the [`reqwest::Error`] is returned so the caller can wrap
/// it in its own adapter error variant (typically `…Error::Http`).
///
/// # Errors
///
/// Returns the underlying [`reqwest::Error`] if the TLS / DNS backend fails to
/// initialize (a broken environment, not a configuration error).
pub fn build_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    default_client_builder().timeout(timeout).build()
}

/// Build a [`reqwest::Client`] that does **not** follow redirects.
///
/// Used by `web_fetch` to enforce its own same-host redirect policy.
/// Otherwise identical to [`default_client`].
///
/// # Panics
///
/// Panics if the underlying TLS / DNS init fails.
#[must_use]
pub fn no_redirect_client() -> reqwest::Client {
    default_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("no-redirect reqwest::Client builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_contains_pkg_version() {
        let v = env!("CARGO_PKG_VERSION");
        assert!(
            USER_AGENT.contains(v),
            "USER_AGENT {USER_AGENT:?} should contain CARGO_PKG_VERSION {v:?}",
        );
        assert!(
            USER_AGENT.starts_with("caliban/"),
            "USER_AGENT {USER_AGENT:?} should start with caliban/",
        );
    }

    #[test]
    fn default_client_constructs() {
        // Smoke test: the client builds without panicking.
        let _client = default_client();
    }

    #[test]
    fn default_client_builder_constructs() {
        // The builder yields a working client.
        let client = default_client_builder().build().expect("builder builds");
        let _ = client;
    }

    #[test]
    fn no_redirect_client_constructs() {
        // Smoke test: Policy::none() variant builds.
        let _client = no_redirect_client();
    }

    #[test]
    fn build_client_applies_timeout_override() {
        // build_client layers the caller's timeout onto the shared defaults.
        let client = build_client(Duration::from_secs(90)).expect("build_client builds");
        let dbg = format!("{client:?}");
        assert!(
            dbg.contains("90"),
            "expected debug repr {dbg:?} to mention the 90s timeout",
        );
    }

    #[test]
    fn default_client_builder_honors_timeout_override() {
        // Callers must be able to override the default timeout by chaining
        // `.timeout(...)` after `default_client_builder()`.
        let override_timeout = Duration::from_mins(2);
        let client = default_client_builder()
            .timeout(override_timeout)
            .build()
            .expect("builder builds with override");
        // The Debug repr embeds the configured timeout; that's a brittle
        // surface but stable enough for a smoke check that the override
        // path is wired.
        let dbg = format!("{client:?}");
        assert!(
            dbg.contains("120"),
            "expected debug repr {dbg:?} to mention the 120s override",
        );
    }

    #[test]
    fn default_timeout_is_thirty_seconds() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
    }
}
