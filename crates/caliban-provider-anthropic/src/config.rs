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

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::sync::Mutex;

    /// Serializes tests that mutate the shared process environment so they
    /// cannot race each other (Rust runs tests in parallel by default).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Set an env var. `unsafe` in the 2024 edition because it mutates global
    /// process state; the [`ENV_LOCK`] held by [`with_clean_env`] serializes all
    /// such mutations within this test binary, so no concurrent access occurs.
    #[allow(unsafe_code)]
    fn set_env(key: &str, val: &str) {
        unsafe { std::env::set_var(key, val) };
    }

    /// Remove an env var. See [`set_env`] for the safety rationale.
    #[allow(unsafe_code)]
    fn remove_env(key: &str) {
        unsafe { std::env::remove_var(key) };
    }

    /// Snapshot the relevant env vars, run `f` with a clean slate, then restore.
    /// Keeps the global environment hermetic across the whole test binary.
    fn with_clean_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved: Vec<(&str, Option<String>)> = [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_VERSION",
        ]
        .into_iter()
        .map(|k| (k, std::env::var(k).ok()))
        .collect();
        for (k, _) in &saved {
            remove_env(k);
        }
        let result = f();
        for (k, v) in saved {
            match v {
                Some(val) => set_env(k, &val),
                None => remove_env(k),
            }
        }
        result
    }

    #[test]
    fn new_uses_documented_defaults() {
        let cfg = DirectConfig::new(SecretString::new("sk-test".into()));
        assert_eq!(cfg.api_key.expose_secret(), "sk-test");
        assert_eq!(cfg.base_url.as_str(), "https://api.anthropic.com/");
        assert_eq!(cfg.anthropic_version, DEFAULT_ANTHROPIC_VERSION);
        assert_eq!(cfg.anthropic_version, "2023-06-01");
        assert_eq!(cfg.timeout, Duration::from_mins(1));
    }

    #[test]
    fn new_is_cloneable_and_debuggable() {
        let cfg = DirectConfig::new(SecretString::new("sk-abc".into()));
        let cloned = cfg.clone();
        assert_eq!(cloned.api_key.expose_secret(), "sk-abc");
        // SecretString must not leak the key through Debug.
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("sk-abc"), "Debug leaked secret: {dbg}");
    }

    #[test]
    fn from_env_missing_api_key_errors() {
        with_clean_env(|| {
            let err = DirectConfig::from_env().expect_err("missing key should error");
            match err {
                AnthropicError::MissingConfig(field) => {
                    assert_eq!(field, "ANTHROPIC_API_KEY");
                }
                other => panic!("expected MissingConfig, got {other:?}"),
            }
        });
    }

    #[test]
    fn from_env_with_only_api_key_uses_defaults() {
        with_clean_env(|| {
            set_env("ANTHROPIC_API_KEY", "sk-from-env");
            let cfg = DirectConfig::from_env().expect("key present");
            assert_eq!(cfg.api_key.expose_secret(), "sk-from-env");
            assert_eq!(cfg.base_url.as_str(), "https://api.anthropic.com/");
            assert_eq!(cfg.anthropic_version, "2023-06-01");
            assert_eq!(cfg.timeout, Duration::from_mins(1));
        });
    }

    #[test]
    fn from_env_overrides_base_url_and_version() {
        with_clean_env(|| {
            set_env("ANTHROPIC_API_KEY", "sk-x");
            set_env("ANTHROPIC_BASE_URL", "https://proxy.internal:8443/v1");
            set_env("ANTHROPIC_VERSION", "2099-01-01");
            let cfg = DirectConfig::from_env().expect("valid config");
            assert_eq!(cfg.base_url.as_str(), "https://proxy.internal:8443/v1");
            assert_eq!(cfg.anthropic_version, "2099-01-01");
            // API key still threads through.
            assert_eq!(cfg.api_key.expose_secret(), "sk-x");
        });
    }

    #[test]
    fn from_env_invalid_base_url_errors_as_transport() {
        with_clean_env(|| {
            set_env("ANTHROPIC_API_KEY", "sk-x");
            set_env("ANTHROPIC_BASE_URL", "not a valid url");
            let err = DirectConfig::from_env().expect_err("bad url should error");
            match err {
                AnthropicError::Transport(_) => {}
                other => panic!("expected Transport, got {other:?}"),
            }
        });
    }

    #[test]
    fn from_env_empty_version_string_is_taken_verbatim() {
        // An explicitly-set (even empty) ANTHROPIC_VERSION overrides the default,
        // since the code only checks presence, not non-emptiness.
        with_clean_env(|| {
            set_env("ANTHROPIC_API_KEY", "sk-x");
            set_env("ANTHROPIC_VERSION", "");
            let cfg = DirectConfig::from_env().expect("valid config");
            assert_eq!(cfg.anthropic_version, "");
        });
    }

    #[test]
    fn base_url_can_be_mutated_after_new() {
        let mut cfg = DirectConfig::new(SecretString::new("sk".into()));
        cfg.base_url = Url::parse("https://example.test/api").expect("valid");
        assert_eq!(cfg.base_url.host_str(), Some("example.test"));
        assert_eq!(cfg.base_url.path(), "/api");
    }
}

#[cfg(feature = "bedrock")]
pub use bedrock_cfg::*;

#[cfg(feature = "vertex")]
pub use vertex_cfg::*;

#[cfg(feature = "vertex")]
mod vertex_cfg {
    use std::sync::Arc;
    use std::time::Duration;

    use gcp_auth::TokenProvider;

    use crate::error::AnthropicError;

    /// Configuration for Google Vertex AI transport (Anthropic Claude on Vertex).
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
        /// Vertex-required Anthropic API version string.
        pub anthropic_version: String,
    }

    impl std::fmt::Debug for VertexConfig {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("VertexConfig")
                .field("project", &self.project)
                .field("region", &self.region)
                .field("timeout", &self.timeout)
                .field("anthropic_version", &self.anthropic_version)
                .field("token_provider", &"<dyn TokenProvider>")
                .finish()
        }
    }

    impl VertexConfig {
        /// Build from ADC (application default credentials) — env, gcloud, metadata server, etc.
        ///
        /// # Errors
        ///
        /// Returns an error if the `gcp_auth` provider cannot be obtained.
        pub async fn from_gcp_credentials(
            project: impl Into<String>,
            region: impl Into<String>,
        ) -> Result<Self, AnthropicError> {
            let token_provider = gcp_auth::provider()
                .await
                .map_err(|e| AnthropicError::Transport(Box::new(e)))?;
            Ok(Self {
                token_provider,
                project: project.into(),
                region: region.into(),
                timeout: Duration::from_mins(1),
                anthropic_version: "vertex-2023-10-16".to_string(),
            })
        }
    }
}

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
                timeout: Duration::from_mins(1),
                anthropic_version: "bedrock-2023-05-31".to_string(),
            })
        }

        /// Construct from an already-built `SdkConfig` (e.g., when the caller has
        /// pre-loaded credentials or wants to override the region/profile).
        #[must_use]
        pub fn from_sdk_config(sdk_config: SdkConfig) -> Self {
            Self {
                sdk_config,
                timeout: Duration::from_mins(1),
                anthropic_version: "bedrock-2023-05-31".to_string(),
            }
        }
    }
}
