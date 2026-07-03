//! Per-transport configuration structs for the `OpenAI` adapter.

use std::time::Duration;

use secrecy::SecretString;
use url::Url;

use crate::error::OpenAIError;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Configuration for the direct `OpenAI` HTTPS transport.
#[derive(Debug, Clone)]
pub struct DirectConfig {
    /// The API key used for `Authorization: Bearer` authentication.
    pub api_key: SecretString,
    /// The base URL (default: `https://api.openai.com/v1`).
    pub base_url: Url,
    /// Optional `OpenAI` organization identifier.
    pub organization: Option<String>,
    /// Optional `OpenAI` project identifier.
    pub project: Option<String>,
    /// Request timeout (non-streaming `send` path).
    pub timeout: Duration,
    /// Optional **total** timeout for the streaming path. `None` (default) =
    /// no total cap; the stream relies on the connect timeout + the agent-core
    /// `WatchedStream` idle watchdog (#254). `Some(d)` re-imposes a hard
    /// wall-clock cap.
    pub stream_total_timeout: Option<Duration>,
}

impl DirectConfig {
    /// Create a new `DirectConfig` with default settings and the given API key.
    ///
    /// # Panics
    ///
    /// Panics if the static default base URL cannot be parsed (this never happens in practice).
    #[must_use]
    pub fn new(api_key: SecretString) -> Self {
        Self {
            api_key,
            base_url: Url::parse(DEFAULT_BASE_URL).expect("static URL parses"),
            organization: None,
            project: None,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            stream_total_timeout: None,
        }
    }

    /// Load configuration from environment variables.
    ///
    /// Required: `OPENAI_API_KEY`.
    /// Optional: `OPENAI_BASE_URL`, `OPENAI_ORG_ID`, `OPENAI_PROJECT`.
    ///
    /// # Errors
    ///
    /// Returns `Err(OpenAIError::MissingConfig)` if `OPENAI_API_KEY` is not set,
    /// or `Err(OpenAIError::InvalidBaseUrl)` if `OPENAI_BASE_URL` is not a valid URL.
    pub fn from_env() -> Result<Self, OpenAIError> {
        let key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| OpenAIError::MissingConfig("OPENAI_API_KEY".into()))?;
        let url = std::env::var("OPENAI_BASE_URL").ok();
        let mut cfg = Self::from_parts(
            SecretString::new(key.into()),
            url.as_deref(),
            std::env::var("OPENAI_ORG_ID").ok(),
            std::env::var("OPENAI_PROJECT").ok(),
        )?;
        cfg.organization = std::env::var("OPENAI_ORG_ID").ok();
        cfg.project = std::env::var("OPENAI_PROJECT").ok();
        Ok(cfg)
    }

    /// Build a `DirectConfig` from explicit parts. Exposed so the env-
    /// reading and URL-parsing branches can be exercised independently
    /// in tests (mirrors `caliban_provider_ollama::config::from_env_value`).
    ///
    /// `base_url == None` selects the default; a `Some(value)` that does
    /// not parse as a URL returns `Err(OpenAIError::InvalidBaseUrl { … })`
    /// so the operator sees the env var name in the surface line instead
    /// of the bare `url::ParseError` text.
    ///
    /// # Errors
    ///
    /// Returns `Err(OpenAIError::InvalidBaseUrl)` when `base_url` is
    /// `Some` but not a parseable URL.
    pub fn from_parts(
        api_key: SecretString,
        base_url: Option<&str>,
        organization: Option<String>,
        project: Option<String>,
    ) -> Result<Self, OpenAIError> {
        let mut cfg = Self::new(api_key);
        if let Some(url) = base_url {
            cfg.base_url = Url::parse(url).map_err(|e| OpenAIError::InvalidBaseUrl {
                value: url.to_string(),
                source: Box::new(e),
            })?;
        }
        cfg.organization = organization;
        cfg.project = project;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_key() -> SecretString {
        SecretString::new("sk-test".into())
    }

    #[test]
    fn unset_base_url_falls_back_to_default() {
        let cfg = DirectConfig::from_parts(dummy_key(), None, None, None)
            .expect("unset OPENAI_BASE_URL must succeed");
        assert_eq!(cfg.base_url.as_str(), "https://api.openai.com/v1");
    }

    #[test]
    fn valid_base_url_is_used() {
        let cfg =
            DirectConfig::from_parts(dummy_key(), Some("http://localhost:1234/v1"), None, None)
                .expect("valid URL should succeed");
        assert_eq!(cfg.base_url.as_str(), "http://localhost:1234/v1");
    }

    #[test]
    fn malformed_base_url_returns_invalid_base_url_error() {
        // Regression: F2 from the 2026-05-27 lmstudio probe — a malformed
        // OPENAI_BASE_URL was being wrapped as `OpenAIError::Transport`
        // (an opaque, type-erased box) and the binary's startup dispatch
        // collapsed it into "OPENAI_API_KEY is not set" even when the key
        // was set. The error must be a distinct variant that carries the
        // env-var name so the startup surface line can call it out.
        let err = DirectConfig::from_parts(dummy_key(), Some("not://a:url:!@#"), None, None)
            .expect_err("malformed URL must error");
        match err {
            OpenAIError::InvalidBaseUrl { value, .. } => {
                assert_eq!(value, "not://a:url:!@#");
            }
            other => panic!("expected InvalidBaseUrl, got {other:?}"),
        }
    }
}

#[cfg(feature = "azure")]
pub use azure::AzureConfig;

#[cfg(feature = "azure")]
mod azure {
    use std::collections::HashMap;
    use std::time::Duration;

    use secrecy::SecretString;
    use url::Url;

    use crate::error::OpenAIError;

    const DEFAULT_TIMEOUT_SECS: u64 = 60;

    /// Configuration for the Azure `OpenAI` transport.
    #[derive(Debug, Clone)]
    pub struct AzureConfig {
        /// The API key for `api-key` header authentication.
        pub api_key: SecretString,
        /// The Azure `OpenAI` resource name (subdomain of `openai.azure.com`).
        pub resource: String,
        /// The Azure `OpenAI` API version (e.g., `"2024-10-21"`).
        pub api_version: String,
        /// Request timeout.
        pub timeout: Duration,
        /// Map from canonical model name to Azure deployment name.
        pub deployments: HashMap<String, String>,
        /// Optional base URL override (used in tests to point at a mock server).
        /// When `None`, the URL is derived from `resource`.
        pub base_url: Option<Url>,
    }

    impl AzureConfig {
        /// Load configuration from environment variables.
        ///
        /// Required: `AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_RESOURCE`.
        /// Optional: `AZURE_OPENAI_API_VERSION` (default `"2024-10-21"`).
        ///
        /// # Errors
        ///
        /// Returns `Err` if required env vars are absent.
        pub fn from_env() -> Result<Self, OpenAIError> {
            let api_key = std::env::var("AZURE_OPENAI_API_KEY")
                .map_err(|_| OpenAIError::MissingConfig("AZURE_OPENAI_API_KEY".into()))?;
            let resource = std::env::var("AZURE_OPENAI_RESOURCE")
                .map_err(|_| OpenAIError::MissingConfig("AZURE_OPENAI_RESOURCE".into()))?;
            let api_version =
                std::env::var("AZURE_OPENAI_API_VERSION").unwrap_or_else(|_| "2024-10-21".into());
            Ok(Self {
                api_key: SecretString::new(api_key.into()),
                resource,
                api_version,
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                deployments: HashMap::new(),
                base_url: None,
            })
        }

        /// Add a mapping from a canonical model name to an Azure deployment name.
        ///
        /// Enables fluent construction:
        /// ```rust,ignore
        /// AzureConfig::from_env()?
        ///     .with_deployment("gpt-4o", "my-gpt-4o-deploy")
        ///     .with_deployment("gpt-4o-mini", "my-mini-deploy")
        /// ```
        #[must_use]
        pub fn with_deployment(
            mut self,
            canonical_model: impl Into<String>,
            deployment: impl Into<String>,
        ) -> Self {
            self.deployments
                .insert(canonical_model.into(), deployment.into());
            self
        }
    }
}
