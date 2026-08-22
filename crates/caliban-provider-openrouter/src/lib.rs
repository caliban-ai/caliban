//! `OpenRouter` provider for the caliban agent harness.
//!
//! [OpenRouter](https://openrouter.ai) is a single `OpenAI`-compatible gateway in
//! front of several hundred models from many vendors. Because the wire format
//! is `OpenAI`'s, this crate is a thin decorator over
//! [`caliban_provider_openai::OpenAIProvider`]: the transport is reused
//! wholesale, and only the three capability-facing trait methods are replaced.
//!
//! # Why a decorator and not `base_url`
//!
//! Pointing the `openai` provider's `base_url` at `OpenRouter` already *reaches*
//! it — but [`caliban_provider_openai::models::capabilities_for`] answers from a
//! static `OpenAI` table and, on a miss, returns a blanket
//! `caps(128_000, 4_096, false, false)` while unconditionally asserting parallel
//! tool use, JSON mode, and automatic prompt caching. No `OpenRouter` id is in
//! that table, so every model is mis-described at once — understating output
//! limits by up to ~94x, denying vision to models that accept images, and
//! claiming tool use for models that have none (#573).
//!
//! This crate answers from the vendored catalogue instead, and says so out loud
//! when it cannot.

pub mod catalog;
pub mod config;

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, MessageStream, ModelInfo,
    PromptCachingCapability, Provider, Result, SystemPromptCapability, ToolUseCapability,
};
use caliban_provider_openai::{OpenAIProvider, transport::direct::DirectTransport};

pub use catalog::{Catalog, CatalogError};

/// Failure constructing an [`OpenRouterProvider`].
///
/// A library crate, so this is `thiserror` rather than `anyhow` (ADR 0002) —
/// which also lets `caliban_provider::Error::adapter` accept it, since that
/// bound requires `std::error::Error`.
#[derive(Debug, thiserror::Error)]
pub enum OpenRouterError {
    /// The vendored or operator-supplied catalogue could not be loaded.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// The underlying `OpenAI`-shaped transport could not be built.
    #[error(transparent)]
    Transport(#[from] caliban_provider::Error),
}

/// The public model catalogue endpoint. Used only by
/// [`OpenRouterProvider::refresh_models`]; the offline path is the vendored
/// snapshot.
pub const MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// `OpenRouter` provider: `OpenAI` transport, `OpenRouter` catalogue.
pub struct OpenRouterProvider {
    inner: OpenAIProvider<DirectTransport>,
    catalog: Catalog,
    /// Models we have already warned about, so an unknown id in a hot loop
    /// produces one warning per session rather than one per request. Mirrors
    /// the rate-card posture in ADR 0033.
    warned: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for OpenRouterProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterProvider")
            .field("models", &self.catalog.len())
            .finish_non_exhaustive()
    }
}

impl OpenRouterProvider {
    /// Build from an `OpenAI`-shaped direct config, loading the catalogue from
    /// `CALIBAN_OPENROUTER_MODELS` when set and the embedded snapshot otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if the transport cannot be constructed or the
    /// catalogue snapshot is unreadable/unparseable.
    pub fn direct(
        cfg: caliban_provider_openai::config::DirectConfig,
    ) -> std::result::Result<Self, OpenRouterError> {
        let catalog = Catalog::load()?;
        let inner = OpenAIProvider::direct(cfg)?;
        Ok(Self {
            inner,
            catalog,
            warned: Mutex::new(HashSet::new()),
        })
    }

    /// Build with an explicit catalogue — the seam tests use.
    ///
    /// # Errors
    ///
    /// Returns an error if the transport cannot be constructed.
    pub fn with_catalog(
        cfg: caliban_provider_openai::config::DirectConfig,
        catalog: Catalog,
    ) -> std::result::Result<Self, OpenRouterError> {
        Ok(Self {
            inner: OpenAIProvider::direct(cfg)?,
            catalog,
            warned: Mutex::new(HashSet::new()),
        })
    }

    /// The catalogue backing this provider.
    #[must_use]
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Capabilities used when the catalogue does not describe a model.
    ///
    /// Every optional capability is off. An unknown model gets the smallest
    /// safe surface rather than an optimistic guess, and
    /// [`Self::capabilities`] warns whenever this is reached.
    fn unknown_model_caps() -> Capabilities {
        Capabilities {
            max_input_tokens: 8_192,
            max_output_tokens: 4_096,
            vision: false,
            tool_use: ToolUseCapability::None,
            thinking: false,
            prompt_caching: PromptCachingCapability::None,
            json_mode: false,
            streaming: true,
            stop_sequences: false,
            top_k: false,
            system_prompt: SystemPromptCapability::SystemRole,
            refusal_field: false,
        }
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        self.inner.complete(req).await
    }

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        self.inner.stream(req).await
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        if let Some(caps) = self.catalog.capabilities_for(model) {
            return caps;
        }
        let first_time = self
            .warned
            .lock()
            .map_or(true, |mut seen| seen.insert(model.to_string()));
        if first_time {
            tracing::warn!(
                target: "caliban::provider::openrouter",
                model,
                catalog_models = self.catalog.len(),
                "model is not in the vendored OpenRouter catalogue; using minimal \
                 capabilities (no tools, no vision, no reasoning). Refresh with \
                 scripts/refresh-openrouter-models.sh, or point {} at a newer snapshot",
                catalog::SNAPSHOT_ENV,
            );
        }
        Self::unknown_model_caps()
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        self.catalog.model_infos()
    }

    async fn refresh_models(&self) -> Result<Vec<ModelInfo>> {
        let raw = reqwest::get(MODELS_URL)
            .await
            .map_err(caliban_provider::Error::adapter)?
            .error_for_status()
            .map_err(caliban_provider::Error::adapter)?
            .text()
            .await
            .map_err(caliban_provider::Error::adapter)?;
        let live = Catalog::from_live_json(&raw).map_err(caliban_provider::Error::adapter)?;
        Ok(live.model_infos())
    }

    fn name(&self) -> &'static str {
        "openrouter"
    }
}
