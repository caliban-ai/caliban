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

#[cfg(feature = "bedrock")]
pub use bedrock_cfg::*;

#[cfg(feature = "bedrock")]
mod bedrock_cfg {
    use std::time::Duration;

    use aws_config::SdkConfig;

    use crate::error::AnthropicError;

    /// Configuration for AWS Bedrock transport (Claude on Bedrock).
    #[derive(Debug, Clone)]
    pub struct BedrockConfig {
        /// AWS SDK config holding credentials, region, retry policy.
        pub sdk_config: SdkConfig,
        /// Request timeout.
        pub timeout: Duration,
        /// Bedrock-required Anthropic API version string.
        pub anthropic_version: String,
    }

    impl BedrockConfig {
        /// Build a `BedrockConfig` from the default AWS credential provider chain
        /// (env, profile, IMDS, etc.).
        ///
        /// # Errors
        ///
        /// Currently infallible. Async because credential loading is async.
        pub async fn from_aws_credentials() -> Result<Self, AnthropicError> {
            let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await;
            Ok(Self {
                sdk_config,
                timeout: Duration::from_secs(60),
                anthropic_version: "bedrock-2023-05-31".to_string(),
            })
        }

        /// Construct from an already-built `SdkConfig` (e.g., when the caller has
        /// pre-loaded credentials or wants to override the region/profile).
        #[must_use]
        pub fn from_sdk_config(sdk_config: SdkConfig) -> Self {
            Self {
                sdk_config,
                timeout: Duration::from_secs(60),
                anthropic_version: "bedrock-2023-05-31".to_string(),
            }
        }
    }
}
