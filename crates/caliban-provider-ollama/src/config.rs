//! Per-transport configuration structs for the Ollama adapter.

use std::time::Duration;

use url::Url;

use crate::error::OllamaError;

const DEFAULT_BASE_URL: &str = "http://localhost:11434";
/// Local models can be slow to load, so use a generous timeout.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Configuration for the direct Ollama HTTP transport.
#[derive(Debug, Clone)]
pub struct DirectConfig {
    /// The base URL of the Ollama server (default: `http://localhost:11434`).
    pub base_url: Url,
    /// Request timeout.
    pub timeout: Duration,
}

impl DirectConfig {
    /// Create a new `DirectConfig` pointing to the default local Ollama instance.
    ///
    /// # Panics
    ///
    /// Panics if the static default base URL cannot be parsed (this never happens in practice).
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_url: Url::parse(DEFAULT_BASE_URL).expect("static URL parses"),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    /// Alias for [`Self::new`] — returns a config targeting local Ollama.
    #[must_use]
    pub fn local() -> Self {
        Self::new()
    }

    /// Load configuration from environment variables.
    ///
    /// Optional: `OLLAMA_BASE_URL` (defaults to `http://localhost:11434`).
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError::Transport)` if `OLLAMA_BASE_URL` is set to
    /// a value that is not a valid URL. An unset env var yields a config
    /// targeting the local default.
    pub fn from_env() -> Result<Self, OllamaError> {
        Self::from_env_value(std::env::var("OLLAMA_BASE_URL").ok().as_deref())
    }

    /// Build a config from an explicit `OLLAMA_BASE_URL` value (or `None`
    /// to represent the unset case). Used internally by [`Self::from_env`]
    /// and exposed for tests so the env-reading and URL-parsing branches
    /// can be exercised independently.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError::Transport)` if the value is `Some` but
    /// not a parseable URL.
    pub fn from_env_value(url: Option<&str>) -> Result<Self, OllamaError> {
        let mut cfg = Self::new();
        if let Some(url) = url {
            cfg.base_url = Url::parse(url).map_err(|e| OllamaError::Transport(Box::new(e)))?;
        }
        Ok(cfg)
    }
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_env_falls_back_to_local() {
        let cfg = DirectConfig::from_env_value(None).expect("unset should succeed");
        // `Url::parse` adds the trailing slash for an authority-only URL.
        assert_eq!(cfg.base_url.as_str(), "http://localhost:11434/");
    }

    #[test]
    fn valid_env_url_is_used() {
        let cfg = DirectConfig::from_env_value(Some("https://example.com:8443"))
            .expect("valid URL should succeed");
        assert_eq!(cfg.base_url.as_str(), "https://example.com:8443/");
    }

    #[test]
    fn malformed_env_url_returns_error() {
        // Regression: previously the binary collapsed this into a silent
        // fallback to localhost (`unwrap_or_else(|_| local())`); operators
        // who mistyped their URL would see a "connection refused to
        // localhost" error rather than a hint that their config was wrong.
        let err = DirectConfig::from_env_value(Some("not://a:url:!@#"))
            .expect_err("malformed URL must error");
        assert!(matches!(err, OllamaError::Transport(_)));
    }
}
