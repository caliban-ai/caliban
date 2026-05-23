//! Per-transport configuration structs.

use std::time::Duration;

use secrecy::SecretString;
use url::Url;

use crate::error::AnthropicError;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Configuration for the direct HTTPS transport (`api.anthropic.com`).
#[derive(Debug, Clone)]
pub struct DirectConfig {
    /// The Anthropic API key.
    pub api_key: SecretString,
    /// Base URL (defaults to `https://api.anthropic.com`).
    pub base_url: Url,
    /// Value for the `anthropic-version` header.
    pub anthropic_version: String,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl DirectConfig {
    /// Create a `DirectConfig` with defaults, supplying only the API key.
    ///
    /// # Panics
    ///
    /// Never panics in practice — the internal static URL is always valid.
    #[must_use]
    pub fn new(api_key: SecretString) -> Self {
        Self {
            api_key,
            base_url: Url::parse(DEFAULT_BASE_URL).expect("static URL parses"),
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    /// Build a `DirectConfig` from environment variables.
    ///
    /// Reads `ANTHROPIC_API_KEY` (required), `ANTHROPIC_BASE_URL` (optional),
    /// and `ANTHROPIC_VERSION` (optional).
    ///
    /// # Errors
    ///
    /// Returns `Err(AnthropicError::MissingConfig)` if `ANTHROPIC_API_KEY` is absent,
    /// or `Err(AnthropicError::Transport)` if `ANTHROPIC_BASE_URL` is not a valid URL.
    pub fn from_env() -> Result<Self, AnthropicError> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| AnthropicError::MissingConfig("ANTHROPIC_API_KEY"))?;
        let mut cfg = Self::new(SecretString::new(key.into()));
        if let Ok(url) = std::env::var("ANTHROPIC_BASE_URL") {
            cfg.base_url = Url::parse(&url).map_err(|e| AnthropicError::Transport(Box::new(e)))?;
        }
        if let Ok(v) = std::env::var("ANTHROPIC_VERSION") {
            cfg.anthropic_version = v;
        }
        Ok(cfg)
    }
}
