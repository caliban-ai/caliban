//! AWS Bedrock provider for the caliban agent harness.
//!
//! Thin wrapper around `caliban_provider_anthropic::AnthropicProvider<BedrockTransport>`
//! that adds:
//!
//! - A `BedrockConfig` with `from_env` / `from_aws_config` constructors.
//! - An [`AuthRefresh`] background task that periodically refreshes the
//!   AWS SDK credential cache so external rotations (e.g. `aws sso login`)
//!   are picked up.
//! - A `list_models` returning Anthropic models known to be served by
//!   Bedrock. Per ADR 0034 this is a vendored list — calling the AWS
//!   control plane (`ListInferenceProfiles`) is left to a future ADR so
//!   we avoid pulling in `aws-sdk-bedrock`.
//! - `name() -> "bedrock"` so the model router and telemetry attribute
//!   it correctly.
//!
//! See `docs/adr/0034-bedrock-and-vertex-providers.md` and
//! `docs/superpowers/specs/2026-05-24-bedrock-vertex-providers-design.md`.

#![allow(clippy::missing_errors_doc)]
// Transitive AWS-SDK dependencies pull in multiple versions of common crates
// (windows-sys, http-body, etc). These are not under our control.
#![allow(clippy::multiple_crate_versions)]

pub mod auth;
pub mod config;
pub mod error;
pub mod models;

use std::sync::Arc;

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, MessageStream, ModelInfo, Provider, Result,
};
use caliban_provider_anthropic::AnthropicProvider;
use caliban_provider_anthropic::config::BedrockConfig as InnerBedrockConfig;
use caliban_provider_anthropic::transport::bedrock::BedrockTransport;

pub use auth::AuthRefresh;
pub use config::BedrockConfig;
pub use error::BedrockError;

/// Provider that talks to Anthropic Claude on AWS Bedrock.
pub struct BedrockProvider {
    inner: AnthropicProvider<BedrockTransport>,
    config: BedrockConfig,
    auth: Arc<AuthRefresh>,
}

impl std::fmt::Debug for BedrockProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockProvider")
            .field("region", &self.config.region)
            .field("inference_profile_id", &self.config.inference_profile_id)
            .field("auth_refresh", &self.config.auth_refresh)
            .finish_non_exhaustive()
    }
}

impl BedrockProvider {
    /// Build a `BedrockProvider` using the default AWS credential chain
    /// (env, profile, IMDS, SSO, web-identity).
    pub async fn from_env() -> std::result::Result<Self, BedrockError> {
        let cfg = BedrockConfig::from_env()?;
        Self::from_config(cfg).await
    }

    /// Build a `BedrockProvider` from an explicit [`BedrockConfig`].
    pub async fn from_config(cfg: BedrockConfig) -> std::result::Result<Self, BedrockError> {
        let sdk_config = cfg.load_sdk_config().await;
        let inner_cfg = InnerBedrockConfig::from_sdk_config(sdk_config);
        let inner = AnthropicProvider::bedrock(inner_cfg);
        let auth = AuthRefresh::spawn(cfg.auth_refresh);
        Ok(Self {
            inner,
            config: cfg,
            auth: Arc::new(auth),
        })
    }

    /// Access the `AuthRefresh` task (mainly for tests and graceful shutdown).
    #[must_use]
    pub fn auth_refresh(&self) -> &AuthRefresh {
        &self.auth
    }

    /// Access the `BedrockConfig` this provider was constructed with.
    #[must_use]
    pub fn config(&self) -> &BedrockConfig {
        &self.config
    }
}

#[async_trait]
impl Provider for BedrockProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        self.inner.complete(req).await
    }

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        self.inner.stream(req).await
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        // Strip platform-specific prefix/suffix so we look up by base model id.
        let base = models::strip_platform_suffix(model);
        caliban_provider_anthropic::models::capabilities_for(&base)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        models::vendored_bedrock_models()
    }

    fn name(&self) -> &'static str {
        "bedrock"
    }
}
