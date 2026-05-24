//! Google Vertex AI provider for the caliban agent harness.
//!
//! Thin wrapper around `caliban_provider_anthropic::AnthropicProvider<VertexTransport>`
//! that adds:
//!
//! - A [`VertexConfig`] with a `from_env` constructor.
//! - An [`AuthRefresh`] background task that periodically refreshes the
//!   GCP bearer token via `gcp_auth::TokenProvider`.
//! - A `list_models` that calls
//!   `https://{region}-aiplatform.googleapis.com/v1/publishers/anthropic/models`
//!   and falls back to a vendored list on failure.
//! - `name() -> "vertex"` so the model router and telemetry attribute it
//!   correctly.
//!
//! See `adrs/0034-bedrock-and-vertex-providers.md` and
//! `docs/superpowers/specs/2026-05-24-bedrock-vertex-providers-design.md`.

#![allow(clippy::missing_errors_doc)]
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
use caliban_provider_anthropic::config::VertexConfig as InnerVertexConfig;
use caliban_provider_anthropic::transport::vertex::VertexTransport;
use gcp_auth::TokenProvider;

pub use auth::AuthRefresh;
pub use config::VertexConfig;
pub use error::VertexError;

/// Provider that talks to Anthropic Claude on Google Vertex AI.
pub struct VertexProvider {
    inner: AnthropicProvider<VertexTransport>,
    config: VertexConfig,
    token_provider: Arc<dyn TokenProvider>,
    auth: Arc<AuthRefresh>,
    list_client: reqwest::Client,
}

impl std::fmt::Debug for VertexProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexProvider")
            .field("project_id", &self.config.project_id)
            .field("region", &self.config.region)
            .field("auth_refresh", &self.config.auth_refresh)
            .finish_non_exhaustive()
    }
}

impl VertexProvider {
    /// Build a `VertexProvider` from environment variables.
    pub async fn from_env() -> std::result::Result<Self, VertexError> {
        let cfg = VertexConfig::from_env()?;
        Self::from_config(cfg).await
    }

    /// Build a `VertexProvider` from an explicit [`VertexConfig`].
    ///
    /// If `cfg.service_account_key_path` is set, the file is loaded via
    /// `gcp_auth::CustomServiceAccount::from_file`. Otherwise the default
    /// `gcp_auth::provider()` chain is used (ADC, gcloud user creds, GCE
    /// metadata server).
    pub async fn from_config(cfg: VertexConfig) -> std::result::Result<Self, VertexError> {
        let token_provider: Arc<dyn TokenProvider> = if let Some(path) =
            cfg.service_account_key_path.as_deref()
        {
            let sa = gcp_auth::CustomServiceAccount::from_file(path).map_err(VertexError::Auth)?;
            Arc::new(sa)
        } else {
            gcp_auth::provider().await.map_err(VertexError::Auth)?
        };
        Self::from_parts(cfg, token_provider).await
    }

    /// Build a `VertexProvider` with an explicit token provider (mainly
    /// for tests; production callers want `from_env` / `from_config`).
    #[allow(clippy::unused_async)] // async for API symmetry with from_config
    pub async fn from_parts(
        cfg: VertexConfig,
        token_provider: Arc<dyn TokenProvider>,
    ) -> std::result::Result<Self, VertexError> {
        let inner_cfg = InnerVertexConfig {
            token_provider: token_provider.clone(),
            project: cfg.project_id.clone(),
            region: cfg.region.clone(),
            timeout: std::time::Duration::from_mins(1),
            anthropic_version: "vertex-2023-10-16".to_string(),
        };
        let inner = AnthropicProvider::vertex(inner_cfg)
            .map_err(|e| VertexError::Transport(Box::new(e)))?;
        let auth = AuthRefresh::spawn(token_provider.clone(), cfg.auth_refresh);
        let list_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(VertexError::Http)?;
        Ok(Self {
            inner,
            config: cfg,
            token_provider,
            auth: Arc::new(auth),
            list_client,
        })
    }

    /// Access the `AuthRefresh` task (mainly for tests and graceful shutdown).
    #[must_use]
    pub fn auth_refresh(&self) -> &AuthRefresh {
        &self.auth
    }

    /// Access the `VertexConfig` this provider was constructed with.
    #[must_use]
    pub fn config(&self) -> &VertexConfig {
        &self.config
    }

    /// Fetch the live model list from Vertex. On error, returns the
    /// vendored fallback.
    pub async fn list_models_live(&self) -> Vec<ModelInfo> {
        let base = models::default_base_url(&self.config.region);
        match models::list_models_remote(&self.list_client, &self.token_provider, &base).await {
            Ok(models) => models,
            Err(e) => {
                tracing::warn!(
                    target: "caliban::provider::vertex",
                    error = %e,
                    "list_models live fetch failed; falling back to vendored list"
                );
                models::vendored_vertex_models()
            }
        }
    }

    /// Fetch the live model list from Vertex using a caller-supplied base
    /// URL (for tests that point at `wiremock`).
    pub async fn list_models_at(
        &self,
        base_url: &str,
    ) -> std::result::Result<Vec<ModelInfo>, VertexError> {
        models::list_models_remote(&self.list_client, &self.token_provider, base_url).await
    }
}

#[async_trait]
impl Provider for VertexProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        self.inner.complete(req).await
    }

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        self.inner.stream(req).await
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        models::capabilities_for_vertex(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        // The Provider trait is sync; we surface the vendored list here and
        // expose `list_models_live` for callers that want to hit Vertex.
        models::vendored_vertex_models()
    }

    fn name(&self) -> &'static str {
        "vertex"
    }
}
