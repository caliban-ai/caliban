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
    /// Returns `Err(OllamaError::Transport)` if `OLLAMA_BASE_URL` is not a valid URL.
    pub fn from_env() -> Result<Self, OllamaError> {
        let mut cfg = Self::new();
        if let Ok(url) = std::env::var("OLLAMA_BASE_URL") {
            cfg.base_url = Url::parse(&url).map_err(|e| OllamaError::Transport(Box::new(e)))?;
        }
        Ok(cfg)
    }
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self::new()
    }
}
