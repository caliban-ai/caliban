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
    /// Request timeout.
    pub timeout: Duration,
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
    /// or `Err(OpenAIError::Transport)` if `OPENAI_BASE_URL` is not a valid URL.
    pub fn from_env() -> Result<Self, OpenAIError> {
        let key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| OpenAIError::MissingConfig("OPENAI_API_KEY".into()))?;
        let mut cfg = Self::new(SecretString::new(key.into()));
        if let Ok(url) = std::env::var("OPENAI_BASE_URL") {
            cfg.base_url = Url::parse(&url).map_err(|e| OpenAIError::Transport(Box::new(e)))?;
        }
        cfg.organization = std::env::var("OPENAI_ORG_ID").ok();
        cfg.project = std::env::var("OPENAI_PROJECT").ok();
        Ok(cfg)
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
