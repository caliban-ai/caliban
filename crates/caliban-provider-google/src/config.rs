//! Per-transport configuration structs for the Google Gemini adapter.

use std::time::Duration;

use secrecy::SecretString;
use url::Url;

use crate::error::GoogleError;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_API_VERSION: &str = "v1beta";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Configuration for the Google AI Studio HTTPS transport.
#[derive(Debug, Clone)]
pub struct AIStudioConfig {
    /// The API key provided as a URL query parameter.
    pub api_key: SecretString,
    /// The base URL (default: `https://generativelanguage.googleapis.com`).
    pub base_url: Url,
    /// The API version path segment (default: `v1beta`).
    pub api_version: String,
    /// Request timeout.
    pub timeout: Duration,
}

impl AIStudioConfig {
    /// Create a new `AIStudioConfig` with default settings and the given API key.
    ///
    /// # Panics
    ///
    /// Panics if the static default base URL cannot be parsed (this never happens in practice).
    #[must_use]
    pub fn new(api_key: SecretString) -> Self {
        Self {
            api_key,
            base_url: Url::parse(DEFAULT_BASE_URL).expect("static URL parses"),
            api_version: DEFAULT_API_VERSION.to_string(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    /// Load configuration from environment variables.
    ///
    /// Required: `GEMINI_API_KEY` or `GOOGLE_GEMINI_API_KEY` (checked in that order).
    /// Optional: `GEMINI_BASE_URL`, `GEMINI_API_VERSION`.
    ///
    /// # Errors
    ///
    /// Returns `Err(GoogleError::MissingConfig)` if no API key env var is set,
    /// or `Err(GoogleError::Transport)` if `GEMINI_BASE_URL` is not a valid URL.
    pub fn from_env() -> Result<Self, GoogleError> {
        let key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_GEMINI_API_KEY"))
            .map_err(|_| GoogleError::MissingConfig("GEMINI_API_KEY or GOOGLE_GEMINI_API_KEY"))?;
        let mut cfg = Self::new(SecretString::new(key.into()));
        if let Ok(url) = std::env::var("GEMINI_BASE_URL") {
            cfg.base_url = Url::parse(&url).map_err(|e| GoogleError::Transport(Box::new(e)))?;
        }
        if let Ok(ver) = std::env::var("GEMINI_API_VERSION") {
            cfg.api_version = ver;
        }
        Ok(cfg)
    }
}
