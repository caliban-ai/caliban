//! Model router — implements [`caliban_provider::Provider`] over a set of
//! purpose-keyed routes that each name a concrete `(provider, model)` pair.
//!
//! See `docs/superpowers/specs/2026-05-23-model-router-design.md` and
//! `adrs/0022-model-routing-architecture.md`.
//!
//! v1 ships:
//! - The TOML config schema (`RouterConfig`, `RouteEntry`) and a builder API.
//! - Purpose-based route resolution (declaration order; first match wins).
//! - `Provider` impl that overrides `request.model` and dispatches to the
//!   resolved adapter for both `complete` and `stream`.
//! - Per-route call/usage tracking.
//!
//! v2 follow-up: fallback chains on fatal-for-route errors, hedged requests,
//! circuit breakers, prompt-cache normalization, capability-based filtering.

#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod error;

pub use config::{RouteEntry, RouterConfig, parse_router_config};
pub use error::{Result, RouterError};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, Error as ProviderError, MessageStream,
    ModelInfo, Provider, RequestPurpose, error::Result as ProviderResult,
};

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Per-route call + usage counts, accumulated by the router.
#[derive(Debug, Clone, Default)]
pub struct RouteUsage {
    /// Number of successful `complete` / `stream` calls.
    pub call_count: u64,
    /// Sum of input tokens across successful calls.
    pub input_tokens: u64,
    /// Sum of output tokens across successful calls.
    pub output_tokens: u64,
    /// Number of calls that ended in a provider error.
    pub failures: u64,
}

/// Snapshot of the router's per-route stats, suitable for rendering.
#[derive(Debug, Clone, Default)]
pub struct RouterStatsSnapshot {
    /// Map of `(provider_name, model)` → usage counters.
    pub per_route: HashMap<(String, String), RouteUsage>,
}

#[derive(Debug, Default)]
struct StatsInner {
    per_route: HashMap<(String, String), RouteUsage>,
}

#[derive(Debug, Clone, Default)]
struct StatsHandle(Arc<Mutex<StatsInner>>);

impl StatsHandle {
    fn record_success(&self, provider: &str, model: &str, input: u32, output: u32) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard
            .per_route
            .entry((provider.to_string(), model.to_string()))
            .or_default();
        entry.call_count += 1;
        entry.input_tokens += u64::from(input);
        entry.output_tokens += u64::from(output);
    }

    fn record_failure(&self, provider: &str, model: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard
            .per_route
            .entry((provider.to_string(), model.to_string()))
            .or_default();
        entry.failures += 1;
    }

    fn snapshot(&self) -> RouterStatsSnapshot {
        let guard = self.0.lock().expect("router stats lock poisoned");
        RouterStatsSnapshot {
            per_route: guard.per_route.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Router that implements [`Provider`] by delegating to inner adapters per
/// route, picked from `request.metadata.purpose`.
pub struct ModelRouter {
    default_purpose: RequestPurpose,
    routes: Vec<RouteEntry>,
    providers: HashMap<String, Arc<dyn Provider + Send + Sync>>,
    stats: StatsHandle,
}

impl std::fmt::Debug for ModelRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRouter")
            .field("default_purpose", &self.default_purpose)
            .field("routes", &self.routes.len())
            .field("providers", &self.providers.len())
            .finish_non_exhaustive()
    }
}

impl ModelRouter {
    /// Start a fresh builder. Default purpose defaults to `MainLoop`; override
    /// with [`ModelRouterBuilder::default_purpose`].
    #[must_use]
    pub fn builder() -> ModelRouterBuilder {
        ModelRouterBuilder::default()
    }

    /// Build a router from a parsed [`RouterConfig`] and a provider map.
    ///
    /// # Errors
    /// See [`RouterError`].
    pub fn from_config(
        cfg: RouterConfig,
        providers: HashMap<String, Arc<dyn Provider + Send + Sync>>,
    ) -> Result<Self> {
        Self::new(cfg.default_purpose, cfg.routes, providers)
    }

    fn new(
        default_purpose: RequestPurpose,
        routes: Vec<RouteEntry>,
        providers: HashMap<String, Arc<dyn Provider + Send + Sync>>,
    ) -> Result<Self> {
        if providers.is_empty() {
            return Err(RouterError::EmptyProviders);
        }
        for r in &routes {
            if !providers.contains_key(&r.provider) {
                return Err(RouterError::UnknownProvider(r.provider.clone()));
            }
        }
        if !routes.iter().any(|r| r.purpose == default_purpose) {
            return Err(RouterError::DefaultPurposeUnrouted(default_purpose));
        }
        Ok(Self {
            default_purpose,
            routes,
            providers,
            stats: StatsHandle::default(),
        })
    }

    /// Return a snapshot of per-route usage stats.
    #[must_use]
    pub fn stats(&self) -> RouterStatsSnapshot {
        self.stats.snapshot()
    }

    /// Resolve the route matching `purpose`. Returns the first entry in
    /// declaration order whose `purpose` matches. Falls back to the first
    /// entry matching `default_purpose` when the request doesn't specify.
    fn resolve(&self, purpose: Option<RequestPurpose>) -> Option<&RouteEntry> {
        let want = purpose.unwrap_or(self.default_purpose);
        self.routes.iter().find(|r| r.purpose == want).or_else(|| {
            self.routes
                .iter()
                .find(|r| r.purpose == self.default_purpose)
        })
    }

    fn prepare_request(
        &self,
        mut request: CompletionRequest,
    ) -> ProviderResult<(CompletionRequest, &RouteEntry)> {
        let purpose = request.metadata.purpose;
        let Some(route) = self.resolve(purpose) else {
            return Err(ProviderError::InvalidRequest(format!(
                "router: no route for purpose {:?} and no default fallback",
                purpose.unwrap_or(self.default_purpose)
            )));
        };
        // Override the caller's model with the route's model so adapters dispatch correctly.
        request.model.clone_from(&route.model);
        Ok((request, route))
    }
}

#[async_trait]
impl Provider for ModelRouter {
    async fn complete(&self, request: CompletionRequest) -> ProviderResult<CompletionResponse> {
        let (request, route) = self.prepare_request(request)?;
        let provider = self
            .providers
            .get(&route.provider)
            .expect("validated at build");
        let provider_name = route.provider.clone();
        let model = route.model.clone();
        match provider.complete(request).await {
            Ok(resp) => {
                self.stats.record_success(
                    &provider_name,
                    &model,
                    resp.usage.input_tokens,
                    resp.usage.output_tokens,
                );
                Ok(resp)
            }
            Err(e) => {
                self.stats.record_failure(&provider_name, &model);
                Err(e)
            }
        }
    }

    async fn stream(&self, request: CompletionRequest) -> ProviderResult<MessageStream> {
        let (request, route) = self.prepare_request(request)?;
        let provider = self
            .providers
            .get(&route.provider)
            .expect("validated at build");
        let provider_name = route.provider.clone();
        let model = route.model.clone();
        match provider.stream(request).await {
            Ok(s) => {
                // We can't easily intercept per-event usage from a stream here without
                // wrapping the stream. v1: record the call as success when the stream
                // is created; usage accumulation lives at the agent layer.
                self.stats.record_success(&provider_name, &model, 0, 0);
                Ok(s)
            }
            Err(e) => {
                self.stats.record_failure(&provider_name, &model);
                Err(e)
            }
        }
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        // Use the resolver with no purpose → the route matching default_purpose.
        // Build-time validation guarantees both lookups succeed.
        let route = self
            .resolve(None)
            .expect("router build validates default_purpose has a route");
        let provider = self
            .providers
            .get(&route.provider)
            .expect("router build validates each route's provider is registered");
        let m = if model.is_empty() {
            route.model.as_str()
        } else {
            model
        };
        provider.capabilities(m)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        // Aggregate models from all registered providers, deduping by id.
        let mut by_id: HashMap<String, ModelInfo> = HashMap::new();
        for p in self.providers.values() {
            for m in p.list_models() {
                by_id.entry(m.id.clone()).or_insert(m);
            }
        }
        by_id.into_values().collect()
    }

    fn name(&self) -> &'static str {
        "router"
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Fluent builder for [`ModelRouter`].
#[derive(Default)]
pub struct ModelRouterBuilder {
    default_purpose: Option<RequestPurpose>,
    routes: Vec<RouteEntry>,
    providers: HashMap<String, Arc<dyn Provider + Send + Sync>>,
}

impl std::fmt::Debug for ModelRouterBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRouterBuilder")
            .field("default_purpose", &self.default_purpose)
            .field("routes", &self.routes.len())
            .field("providers", &self.providers.len())
            .finish_non_exhaustive()
    }
}

impl ModelRouterBuilder {
    /// Set the purpose used when a request specifies none.
    #[must_use]
    pub fn default_purpose(mut self, p: RequestPurpose) -> Self {
        self.default_purpose = Some(p);
        self
    }

    /// Register a provider under a logical name.
    #[must_use]
    pub fn add_provider(
        mut self,
        name: impl Into<String>,
        provider: Arc<dyn Provider + Send + Sync>,
    ) -> Self {
        self.providers.insert(name.into(), provider);
        self
    }

    /// Append a route entry.
    #[must_use]
    pub fn route(
        mut self,
        purpose: RequestPurpose,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.routes.push(RouteEntry {
            purpose,
            provider: provider.into(),
            model: model.into(),
        });
        self
    }

    /// Build the router. See [`RouterError`] for failure modes.
    ///
    /// # Errors
    /// See [`RouterError`].
    pub fn build(self) -> Result<ModelRouter> {
        let default_purpose = self.default_purpose.unwrap_or(RequestPurpose::MainLoop);
        ModelRouter::new(default_purpose, self.routes, self.providers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::MockProvider;

    fn make_mock() -> Arc<dyn Provider + Send + Sync> {
        Arc::new(MockProvider::new())
    }

    #[test]
    fn build_validates_provider_references() {
        let err = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "x")
            .route(RequestPurpose::Summarization, "openai", "y") // unknown
            .build()
            .unwrap_err();
        assert!(matches!(err, RouterError::UnknownProvider(p) if p == "openai"));
    }

    #[test]
    fn build_rejects_empty_providers() {
        let err = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .route(RequestPurpose::MainLoop, "any", "x")
            .build()
            .unwrap_err();
        assert!(matches!(err, RouterError::EmptyProviders));
    }

    #[test]
    fn build_rejects_default_purpose_with_no_route() {
        let err = ModelRouter::builder()
            .default_purpose(RequestPurpose::SubAgent)
            .add_provider("anthropic", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "x")
            .build()
            .unwrap_err();
        assert!(matches!(err, RouterError::DefaultPurposeUnrouted(_)));
    }

    #[test]
    fn resolve_uses_explicit_purpose_when_set() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .add_provider("openai", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "claude")
            .route(RequestPurpose::Summarization, "openai", "gpt")
            .build()
            .unwrap();
        let entry = r.resolve(Some(RequestPurpose::Summarization)).unwrap();
        assert_eq!(entry.provider, "openai");
        assert_eq!(entry.model, "gpt");
    }

    #[test]
    fn resolve_falls_back_to_default_purpose_when_request_has_none() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "claude")
            .build()
            .unwrap();
        let entry = r.resolve(None).unwrap();
        assert_eq!(entry.purpose, RequestPurpose::MainLoop);
    }

    #[test]
    fn resolve_falls_back_to_default_purpose_when_purpose_not_routed() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "claude")
            .build()
            .unwrap();
        // Embedding has no explicit route; should fall back to MainLoop's.
        let entry = r.resolve(Some(RequestPurpose::Embedding)).unwrap();
        assert_eq!(entry.purpose, RequestPurpose::MainLoop);
    }

    #[test]
    fn first_match_wins_for_same_purpose() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .add_provider("openai", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "primary")
            .route(RequestPurpose::MainLoop, "openai", "fallback")
            .build()
            .unwrap();
        let entry = r.resolve(Some(RequestPurpose::MainLoop)).unwrap();
        assert_eq!(entry.model, "primary");
    }

    #[test]
    fn name_returns_router() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "claude")
            .build()
            .unwrap();
        assert_eq!(r.name(), "router");
    }
}
