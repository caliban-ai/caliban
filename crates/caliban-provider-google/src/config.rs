//! Per-transport configuration structs for the Google Gemini adapter.

#[cfg(feature = "vertex")]
pub use vertex_cfg::*;

#[cfg(feature = "vertex")]
mod vertex_cfg {
    use std::sync::Arc;
    use std::time::Duration;

    use gcp_auth::TokenProvider;

    use crate::error::GoogleError;

    /// Configuration for Google Vertex AI transport (Gemini on Vertex).
    #[derive(Clone)]
    pub struct VertexConfig {
        /// GCP token provider (handles refresh).
        pub token_provider: Arc<dyn TokenProvider>,
        /// GCP project ID.
        pub project: String,
        /// GCP region (e.g., `"us-central1"`).
        pub region: String,
        /// Request timeout.
        pub timeout: Duration,
    }

    impl std::fmt::Debug for VertexConfig {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("VertexConfig")
                .field("project", &self.project)
                .field("region", &self.region)
                .field("timeout", &self.timeout)
                .field("token_provider", &"<dyn TokenProvider>")
                .finish()
        }
    }

    impl VertexConfig {
        /// Build from Application Default Credentials.
        ///
        /// # Errors
        ///
        /// Returns an error if the `gcp_auth` provider cannot be obtained.
        pub async fn from_gcp_credentials(
            project: impl Into<String>,
            region: impl Into<String>,
        ) -> Result<Self, GoogleError> {
            let token_provider = gcp_auth::provider()
                .await
                .map_err(|e| GoogleError::Transport(Box::new(e)))?;
            Ok(Self {
                token_provider,
                project: project.into(),
                region: region.into(),
                timeout: Duration::from_secs(60),
            })
        }
    }
}

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
