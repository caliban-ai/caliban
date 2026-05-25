//! Model router — implements [`caliban_provider::Provider`] over a set of
//! routes that each name a concrete `(provider, model)` pair, with v2
//! resilience features layered on top:
//!
//! - Resolution / dispatch split with a candidate-vec seam ([`resolver`]).
//! - Sequential fallback on fatal-for-route errors ([`fallback`]).
//! - Per-route hedging via [`hedging`] (race after `hedge_after_ms`).
//! - Per-route circuit breakers ([`breaker`]).
//! - Capability-based filtering ([`capabilities`]).
//! - Prompt-cache marker stripping on cross-route hops ([`cache`]).
//! - `caliban.toml` discovery ([`discovery`]).
//! - Effort-level resolution ([`effort`]).
//!
//! See `docs/superpowers/specs/2026-05-24-model-router-v2-design.md` and
//! `adrs/0038-model-router-v2.md`.

#![allow(clippy::multiple_crate_versions)]
// Internal dispatcher helpers + tests intentionally exceed pedantic line /
// arg-count caps; the alternatives (splitting hot loops into many small fns
// or threading bespoke types just to satisfy lints) make the code harder
// to follow without buying anything.
#![allow(
    clippy::too_many_lines,
    clippy::redundant_else,
    clippy::single_match_else,
    clippy::map_unwrap_or,
    clippy::doc_markdown,
    clippy::missing_panics_doc,
    clippy::implicit_hasher,
    clippy::ignored_unit_patterns,
    clippy::manual_let_else,
    clippy::needless_continue,
    clippy::cast_possible_truncation,
    clippy::duration_suboptimal_units,
    clippy::default_trait_access,
    clippy::int_plus_one,
    clippy::unreadable_literal
)]

pub mod breaker;
pub mod cache;
pub mod capabilities;
pub mod config;
pub mod discovery;
pub mod effort;
pub mod error;
pub mod fallback;
pub mod hedging;
pub mod resolver;

pub use breaker::{BreakerSnapshot, BreakerState, CircuitBreaker};
pub use capabilities::{CandidateAnnotation, CandidateOrigin, DerivedNeeds};
pub use config::{
    BreakerPolicy, CalibanConfig, CapabilityRequirements, EffortLevel, EffortMap, HedgePolicy,
    ProviderBlock, RouteEntry, RouterConfig, parse_caliban_config, parse_router_config,
};
pub use discovery::{DiscoveredConfig, DiscoveryError, discover_caliban_toml};
pub use effort::{effective_effort_for, effort_knob_for};
pub use error::{Result, RouterError};
pub use fallback::is_fatal_for_route;
pub use resolver::{Candidate, DiagnosticEntry, resolve_candidates, resolve_with_diagnostics};

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
    /// Sum of cache-read input tokens across successful calls.
    pub cache_read_input_tokens: u64,
    /// Sum of cache-creation input tokens across successful calls.
    pub cache_creation_input_tokens: u64,
    /// Number of calls that ended in a provider error.
    pub failures: u64,
    /// Number of times this route was the loser of a hedge race.
    pub hedge_losses: u64,
    /// Number of times this route was the winner of a hedge race.
    pub hedge_wins: u64,
    /// Number of times fallback engaged from this route to another.
    pub fallback_engaged: u64,
}

/// Snapshot of the router's per-route stats.
#[derive(Debug, Clone, Default)]
pub struct RouterStatsSnapshot {
    /// Map of route id → usage counters.
    pub per_route: HashMap<String, RouteUsage>,
    /// Per-route circuit-breaker snapshots.
    pub breakers: HashMap<String, BreakerSnapshot>,
    /// Number of prompt-cache markers cleared during cross-route hops.
    pub cache_markers_cleared: u64,
}

#[derive(Debug, Default)]
struct StatsInner {
    per_route: HashMap<String, RouteUsage>,
    cache_markers_cleared: u64,
}

#[derive(Debug, Clone, Default)]
struct StatsHandle(Arc<Mutex<StatsInner>>);

impl StatsHandle {
    fn record_success(&self, route_id: &str, usage: caliban_provider::Usage) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.call_count += 1;
        entry.input_tokens += u64::from(usage.input_tokens);
        entry.output_tokens += u64::from(usage.output_tokens);
        entry.cache_read_input_tokens += u64::from(usage.cache_read_input_tokens.unwrap_or(0));
        entry.cache_creation_input_tokens +=
            u64::from(usage.cache_creation_input_tokens.unwrap_or(0));
    }

    fn record_failure(&self, route_id: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.failures += 1;
    }

    fn record_hedge_loss(&self, route_id: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.hedge_losses += 1;
    }

    fn record_hedge_win(&self, route_id: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.hedge_wins += 1;
    }

    fn record_fallback_engaged(&self, from: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(from.to_string()).or_default();
        entry.fallback_engaged += 1;
    }

    fn record_cache_markers_cleared(&self, n: u32) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        guard.cache_markers_cleared += u64::from(n);
    }

    fn snapshot(&self, breakers: &HashMap<String, CircuitBreaker>) -> RouterStatsSnapshot {
        let guard = self.0.lock().expect("router stats lock poisoned");
        let breaker_snaps: HashMap<String, BreakerSnapshot> = breakers
            .iter()
            .map(|(k, v)| (k.clone(), v.snapshot()))
            .collect();
        RouterStatsSnapshot {
            per_route: guard.per_route.clone(),
            breakers: breaker_snaps,
            cache_markers_cleared: guard.cache_markers_cleared,
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Router that implements [`Provider`] by dispatching to inner adapters
/// according to the v2 resolution + dispatch pipeline.
pub struct ModelRouter {
    default_purpose: RequestPurpose,
    routes: Vec<RouteEntry>,
    providers: Arc<HashMap<String, Arc<dyn Provider + Send + Sync>>>,
    breakers: Arc<HashMap<String, CircuitBreaker>>,
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
    /// Start a fresh builder. Default purpose defaults to `MainLoop`.
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
        // Validate that every `fallback` id references an existing route.
        let known_ids: std::collections::HashSet<&str> =
            routes.iter().map(|r| r.id.as_str()).collect();
        for r in &routes {
            if let Some(ids) = r.fallback.as_ref() {
                for fid in ids {
                    if !known_ids.contains(fid.as_str()) {
                        return Err(RouterError::UnknownFallbackId {
                            from: r.id.clone(),
                            missing: fid.clone(),
                        });
                    }
                }
            }
        }
        let breakers: HashMap<String, CircuitBreaker> = routes
            .iter()
            .map(|r| (r.id.clone(), CircuitBreaker::new(r.breaker)))
            .collect();
        Ok(Self {
            default_purpose,
            routes,
            providers: Arc::new(providers),
            breakers: Arc::new(breakers),
            stats: StatsHandle::default(),
        })
    }

    /// The configured default purpose.
    #[must_use]
    pub fn default_purpose(&self) -> RequestPurpose {
        self.default_purpose
    }

    /// All routes in declaration order.
    #[must_use]
    pub fn routes(&self) -> &[RouteEntry] {
        &self.routes
    }

    /// Look up a breaker by route id.
    #[must_use]
    pub fn breaker(&self, route_id: &str) -> Option<&CircuitBreaker> {
        self.breakers.get(route_id)
    }

    /// Return a snapshot of per-route usage stats + breaker state.
    #[must_use]
    pub fn stats(&self) -> RouterStatsSnapshot {
        self.stats.snapshot(&self.breakers)
    }

    /// Resolve candidate routes for a request (pure; no dispatch).
    ///
    /// # Errors
    /// Surfaces [`RouterError::NoCandidate`] / [`RouterError::UnknownFallbackId`].
    pub fn resolve(&self, request: &CompletionRequest) -> Result<Vec<Candidate>> {
        resolve_candidates(
            &self.routes,
            &self.breakers,
            &self.providers,
            self.default_purpose,
            request,
        )
    }

    /// Resolve candidates + return per-route diagnostics.
    ///
    /// # Errors
    /// See [`Self::resolve`].
    pub fn resolve_diagnostics(
        &self,
        request: &CompletionRequest,
    ) -> Result<(Vec<Candidate>, Vec<DiagnosticEntry>)> {
        resolve_with_diagnostics(
            &self.routes,
            &self.breakers,
            &self.providers,
            self.default_purpose,
            request,
        )
    }

    /// Prepare a per-route request: clones the inbound, swaps `model` for
    /// the route's, strips cache markers when crossing a route boundary.
    fn rewrite_for_route(
        &self,
        base: &CompletionRequest,
        route: &RouteEntry,
        cross_route: bool,
    ) -> CompletionRequest {
        let mut req = base.clone();
        req.model.clone_from(&route.model);
        if cross_route {
            let cleared = cache::strip_cache_markers(&mut req);
            if cleared > 0 {
                self.stats.record_cache_markers_cleared(cleared);
            }
        }
        req
    }
}

#[async_trait]
impl Provider for ModelRouter {
    async fn complete(&self, request: CompletionRequest) -> ProviderResult<CompletionResponse> {
        let candidates = self
            .resolve(&request)
            .map_err(RouterError::into_provider_error)?;
        debug_assert!(!candidates.is_empty());

        // Track tried route ids for FallbackExhausted error reporting.
        let mut tried: Vec<String> = Vec::with_capacity(candidates.len());
        let mut last_err: Option<ProviderError> = None;
        let mut i: usize = 0;
        while i < candidates.len() {
            let cand = &candidates[i];
            let route = &self.routes[cand.route_idx];
            let route_id = route.id.clone();
            tried.push(route_id.clone());
            let cross_route = i > 0;

            // Decide hedging policy for this segment.
            let policy = route.hedge;
            let remaining = candidates.len() - i;

            if remaining == 1 || matches!(policy, HedgePolicy::Disabled) {
                // Sequential single-attempt for this segment.
                let req = self.rewrite_for_route(&request, route, cross_route);
                let provider = self
                    .providers
                    .get(&route.provider)
                    .expect("router build validates provider names");
                match provider.complete(req).await {
                    Ok(resp) => {
                        self.stats.record_success(&route_id, resp.usage);
                        self.breakers
                            .get(&route_id)
                            .expect("breaker for route")
                            .observe_success();
                        return Ok(resp);
                    }
                    Err(e) => {
                        self.stats.record_failure(&route_id);
                        self.breakers
                            .get(&route_id)
                            .expect("breaker for route")
                            .observe_failure();
                        if is_fatal_for_route(&e) {
                            tracing::warn!(
                                target: caliban_common::tracing_targets::TARGET_ROUTER,
                                route = %route_id,
                                err = %e,
                                "route fatal — advancing to next candidate",
                            );
                            if i + 1 < candidates.len() {
                                self.stats.record_fallback_engaged(&route_id);
                            }
                            last_err = Some(e);
                            i += 1;
                            continue;
                        }
                        return Err(e);
                    }
                }
            } else {
                // Hedge primary + up to `max_hedges` following candidates.
                let max_hedges = match policy {
                    HedgePolicy::Race { max_hedges, .. } => max_hedges,
                    HedgePolicy::Disabled => 0,
                };
                let segment_len = std::cmp::min(remaining, 1 + max_hedges as usize);
                let segment_indices: Vec<usize> = (i..i + segment_len)
                    .map(|j| candidates[j].route_idx)
                    .collect();

                // Pre-build per-candidate requests so the spawn closure can move them.
                let mut rewritten: Vec<CompletionRequest> = Vec::with_capacity(segment_len);
                for (k, &ridx) in segment_indices.iter().enumerate() {
                    let cross = cross_route || k > 0;
                    rewritten.push(self.rewrite_for_route(&request, &self.routes[ridx], cross));
                }

                let providers = Arc::clone(&self.providers);
                let routes_arc: Arc<Vec<RouteEntry>> = Arc::new(self.routes.clone());
                let segment_indices_arc = Arc::new(segment_indices.clone());
                let rewritten_arc = Arc::new(rewritten);

                let (winner_segment_idx, result, losers) =
                    hedging::race_hedged(policy, segment_len, |k, tok| {
                        let providers = Arc::clone(&providers);
                        let routes_arc = Arc::clone(&routes_arc);
                        let segment_indices_arc = Arc::clone(&segment_indices_arc);
                        let rewritten_arc = Arc::clone(&rewritten_arc);
                        async move {
                            let ridx = segment_indices_arc[k];
                            let route = &routes_arc[ridx];
                            let provider = providers
                                .get(&route.provider)
                                .expect("router build validates provider names")
                                .clone();
                            let req = rewritten_arc[k].clone();
                            tokio::select! {
                                _ = tok.cancelled() => {
                                    Err(ProviderError::Cancelled)
                                }
                                res = provider.complete(req) => res,
                            }
                        }
                    })
                    .await;

                // Record losers (failures, hedge_loss for non-Cancelled, breaker observe).
                for loser in &losers {
                    let ridx = segment_indices[loser.idx];
                    let lid = self.routes[ridx].id.clone();
                    if !matches!(loser.result, Err(ProviderError::Cancelled)) {
                        self.stats.record_failure(&lid);
                        self.breakers.get(&lid).expect("breaker").observe_failure();
                    }
                }

                match result {
                    Ok(resp) => {
                        let winner_ridx = segment_indices[winner_segment_idx];
                        let winner_id = self.routes[winner_ridx].id.clone();
                        self.stats.record_success(&winner_id, resp.usage);
                        self.breakers
                            .get(&winner_id)
                            .expect("breaker for route")
                            .observe_success();
                        if segment_len > 1 {
                            self.stats.record_hedge_win(&winner_id);
                            for (k, _) in segment_indices.iter().enumerate() {
                                if k != winner_segment_idx {
                                    let lid = self.routes[segment_indices[k]].id.clone();
                                    self.stats.record_hedge_loss(&lid);
                                }
                            }
                        }
                        return Ok(resp);
                    }
                    Err(e) => {
                        // Winner failed; record breaker miss for the winner too.
                        let winner_ridx = segment_indices[winner_segment_idx];
                        let winner_id = self.routes[winner_ridx].id.clone();
                        if !matches!(e, ProviderError::Cancelled) {
                            self.stats.record_failure(&winner_id);
                            self.breakers
                                .get(&winner_id)
                                .expect("breaker")
                                .observe_failure();
                        }
                        if is_fatal_for_route(&e) {
                            // Move past the whole hedge segment.
                            if i + segment_len < candidates.len() {
                                self.stats.record_fallback_engaged(&winner_id);
                            }
                            last_err = Some(e);
                            i += segment_len;
                            continue;
                        }
                        return Err(e);
                    }
                }
            }
        }

        // Exhausted.
        let last = last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no error captured".to_string());
        Err(RouterError::FallbackExhausted {
            tried,
            last_error: last,
        }
        .into_provider_error())
    }

    async fn stream(&self, request: CompletionRequest) -> ProviderResult<MessageStream> {
        // Streaming: dispatch to the first viable candidate; on fatal error,
        // fall back. Hedging is disabled for streams in v2 — streams are
        // stateful and racing them complicates cancellation in ways we don't
        // want to ship behind a feature flag.
        let candidates = self
            .resolve(&request)
            .map_err(RouterError::into_provider_error)?;
        let mut tried: Vec<String> = Vec::with_capacity(candidates.len());
        let mut last_err: Option<ProviderError> = None;
        for (i, cand) in candidates.iter().enumerate() {
            let route = &self.routes[cand.route_idx];
            tried.push(route.id.clone());
            let cross_route = i > 0;
            let req = self.rewrite_for_route(&request, route, cross_route);
            let provider = self
                .providers
                .get(&route.provider)
                .expect("router build validates provider names");
            match provider.stream(req).await {
                Ok(s) => {
                    // Usage on streams is folded by the agent layer; record a call.
                    self.stats
                        .record_success(&route.id, caliban_provider::Usage::default());
                    self.breakers
                        .get(&route.id)
                        .expect("breaker")
                        .observe_success();
                    return Ok(s);
                }
                Err(e) => {
                    self.stats.record_failure(&route.id);
                    self.breakers
                        .get(&route.id)
                        .expect("breaker")
                        .observe_failure();
                    if is_fatal_for_route(&e) {
                        if i + 1 < candidates.len() {
                            self.stats.record_fallback_engaged(&route.id);
                        }
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        let last = last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no error captured".to_string());
        Err(RouterError::FallbackExhausted {
            tried,
            last_error: last,
        }
        .into_provider_error())
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        // Pick a route matching the default purpose, then ask its provider.
        let route = self
            .routes
            .iter()
            .find(|r| r.purpose == self.default_purpose)
            .expect("router build validates default_purpose has a route");
        let provider = self
            .providers
            .get(&route.provider)
            .expect("router build validates provider names");
        let m = if model.is_empty() {
            route.model.as_str()
        } else {
            model
        };
        provider.capabilities(m)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
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

    /// Append a route entry (simple form — sets defaults for the v2-only fields).
    #[must_use]
    pub fn route(
        mut self,
        purpose: RequestPurpose,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let provider = provider.into();
        let model = model.into();
        let id = format!(
            "{}:{}:{}",
            &provider,
            &model,
            match purpose {
                RequestPurpose::MainLoop => "main_loop",
                RequestPurpose::Summarization => "summarization",
                RequestPurpose::FastClassifier => "fast_classifier",
                RequestPurpose::SubAgent => "sub_agent",
                RequestPurpose::Embedding => "embedding",
                RequestPurpose::Other => "other",
            }
        );
        self.routes.push(RouteEntry {
            id,
            purpose,
            provider,
            model,
            requires: CapabilityRequirements::default(),
            fallback: None,
            hedge: HedgePolicy::Disabled,
            breaker: BreakerPolicy::disabled(),
            effort: None,
            effort_map: EffortMap::default(),
        });
        self
    }

    /// Append a fully-configured route.
    #[must_use]
    pub fn route_entry(mut self, entry: RouteEntry) -> Self {
        self.routes.push(entry);
        self
    }

    /// Build the router.
    ///
    /// # Errors
    /// See [`RouterError`].
    pub fn build(self) -> Result<ModelRouter> {
        let default_purpose = self.default_purpose.unwrap_or(RequestPurpose::MainLoop);
        ModelRouter::new(default_purpose, self.routes, self.providers)
    }
}

// ---------------------------------------------------------------------------
// Diagnostic rendering
// ---------------------------------------------------------------------------

/// Render diagnostics for `caliban router debug`.
#[must_use]
pub fn render_diagnostics(
    purpose: RequestPurpose,
    needs: DerivedNeeds,
    diagnostics: &[DiagnosticEntry],
) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "purpose={purpose:?}  needs={{{}}}", needs.render());
    for d in diagnostics {
        let mark = if d.kept { "+" } else { "-" };
        let breaker = format!("breaker={}", d.breaker_state);
        let _ = writeln!(s, "  {mark} {} [{}] {breaker}", d.route_id, d.reason);
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{
        CompletionRequest, CompletionResponse, Message, MockProvider, Role, StopReason, Usage,
    };

    fn make_mock() -> Arc<MockProvider> {
        Arc::new(MockProvider::new())
    }

    fn req_main_loop() -> CompletionRequest {
        let mut r = CompletionRequest {
            model: String::new(),
            messages: vec![Message::user_text("hi")],
            tools: vec![],
            tool_choice: caliban_provider::ToolChoice::default(),
            max_tokens: 64,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            metadata: Default::default(),
        };
        r.metadata.purpose = Some(RequestPurpose::MainLoop);
        r
    }

    fn ok_response(model: &str) -> CompletionResponse {
        CompletionResponse {
            id: "id".into(),
            model: model.into(),
            message: Message {
                role: Role::Assistant,
                content: vec![],
            },
            stop_reason: StopReason::EndTurn,
            stop_sequence: None,
            usage: Usage::default(),
        }
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
    fn resolution_returns_ordered_candidate_list_for_implicit_fallback() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .add_provider("openai", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "primary")
            .route(RequestPurpose::MainLoop, "openai", "fallback")
            .build()
            .unwrap();
        let cands = r.resolve(&req_main_loop()).unwrap();
        assert_eq!(cands.len(), 2);
        assert_eq!(r.routes[cands[0].route_idx].model, "primary");
        assert_eq!(r.routes[cands[1].route_idx].model, "fallback");
        assert_eq!(cands[0].annotation.origin, CandidateOrigin::Primary);
        assert_eq!(
            cands[1].annotation.origin,
            CandidateOrigin::ImplicitFallback
        );
    }

    #[test]
    fn resolution_purpose_match_only_v1_behavior_preserved() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", make_mock())
            .add_provider("openai", make_mock())
            .route(RequestPurpose::MainLoop, "anthropic", "claude")
            .route(RequestPurpose::Summarization, "openai", "gpt")
            .build()
            .unwrap();
        let mut req = req_main_loop();
        req.metadata.purpose = Some(RequestPurpose::Summarization);
        let cands = r.resolve(&req).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(r.routes[cands[0].route_idx].model, "gpt");
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

    #[tokio::test(flavor = "current_thread")]
    async fn falls_back_on_model_unavailable() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::ModelUnavailable("nope".into())));
        let secondary = make_mock();
        secondary.enqueue_complete(Ok(ok_response("secondary")));
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("anthropic", primary.clone())
            .add_provider("openai", secondary.clone())
            .route(RequestPurpose::MainLoop, "anthropic", "claude")
            .route(RequestPurpose::MainLoop, "openai", "gpt")
            .build()
            .unwrap();
        let resp = r.complete(req_main_loop()).await.unwrap();
        assert_eq!(resp.model, "secondary");
        let stats = r.stats();
        assert_eq!(stats.per_route["anthropic:claude:main_loop"].failures, 1);
        assert_eq!(
            stats.per_route["anthropic:claude:main_loop"].fallback_engaged,
            1
        );
        assert_eq!(stats.per_route["openai:gpt:main_loop"].call_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn falls_back_on_rate_limit_after_adapter_retries() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::RateLimit { retry_after: None }));
        let secondary = make_mock();
        secondary.enqueue_complete(Ok(ok_response("secondary")));
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", primary)
            .add_provider("b", secondary)
            .route(RequestPurpose::MainLoop, "a", "x")
            .route(RequestPurpose::MainLoop, "b", "y")
            .build()
            .unwrap();
        let resp = r.complete(req_main_loop()).await.unwrap();
        assert_eq!(resp.model, "secondary");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn does_not_fall_back_on_auth_error() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::Auth("bad".into())));
        let secondary = make_mock();
        // If fallback engaged the test fails because secondary has nothing queued.
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", primary)
            .add_provider("b", secondary)
            .route(RequestPurpose::MainLoop, "a", "x")
            .route(RequestPurpose::MainLoop, "b", "y")
            .build()
            .unwrap();
        let err = r.complete(req_main_loop()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn does_not_fall_back_on_content_policy() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::ContentFilter("policy".into())));
        let secondary = make_mock();
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", primary)
            .add_provider("b", secondary)
            .route(RequestPurpose::MainLoop, "a", "x")
            .route(RequestPurpose::MainLoop, "b", "y")
            .build()
            .unwrap();
        let err = r.complete(req_main_loop()).await.unwrap_err();
        assert!(matches!(err, ProviderError::ContentFilter(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exhausted_chain_returns_fallback_exhausted_error() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::ModelUnavailable("a".into())));
        let secondary = make_mock();
        secondary.enqueue_complete(Err(ProviderError::ModelUnavailable("b".into())));
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", primary)
            .add_provider("b", secondary)
            .route(RequestPurpose::MainLoop, "a", "x")
            .route(RequestPurpose::MainLoop, "b", "y")
            .build()
            .unwrap();
        let err = r.complete(req_main_loop()).await.unwrap_err();
        match err {
            ProviderError::ModelUnavailable(msg) => {
                assert!(msg.contains("fallback exhausted"), "msg = {msg}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_candidate_list_returns_clear_error() {
        // Build a router that can't satisfy a vision request because the
        // mock's default capabilities have vision=false.
        let only_text = make_mock();
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", only_text)
            .route(RequestPurpose::MainLoop, "a", "x")
            .build()
            .unwrap();
        let mut req = req_main_loop();
        // Add an image to require vision.
        req.messages = vec![caliban_provider::Message {
            role: Role::User,
            content: vec![caliban_provider::ContentBlock::Image(
                caliban_provider::ImageBlock {
                    source: caliban_provider::ImageSource::Url {
                        url: "https://x/y.png".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                },
            )],
        }];
        let err = r.complete(req).await.unwrap_err();
        let s = err.to_string();
        assert!(s.contains("no candidate"), "got: {s}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn capability_filter_routes_to_vision_capable() {
        // Two routes: first has vision=false (default mock), second has vision=true.
        let no_vision = make_mock();
        let with_vision_mock = make_mock();
        let mut caps = caliban_provider::Capabilities {
            max_input_tokens: 100_000,
            max_output_tokens: 4096,
            vision: true,
            tool_use: caliban_provider::ToolUseCapability::Basic,
            thinking: false,
            prompt_caching: caliban_provider::PromptCachingCapability::None,
            json_mode: false,
            streaming: true,
            stop_sequences: true,
            top_k: false,
            system_prompt: caliban_provider::SystemPromptCapability::SeparateField,
            refusal_field: false,
        };
        with_vision_mock.set_capabilities(caps);
        caps.vision = false;
        no_vision.set_capabilities(caps);
        with_vision_mock.enqueue_complete(Ok(ok_response("vision-target")));
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("nv", no_vision)
            .add_provider("v", with_vision_mock)
            .route(RequestPurpose::MainLoop, "nv", "no-vision")
            .route(RequestPurpose::MainLoop, "v", "with-vision")
            .build()
            .unwrap();
        let mut req = req_main_loop();
        req.messages = vec![caliban_provider::Message {
            role: Role::User,
            content: vec![caliban_provider::ContentBlock::Image(
                caliban_provider::ImageBlock {
                    source: caliban_provider::ImageSource::Url {
                        url: "https://x/y.png".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                },
            )],
        }];
        let resp = r.complete(req).await.unwrap();
        assert_eq!(resp.model, "vision-target");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn capability_filter_thinking_falls_back_to_capable() {
        let no_thinking = make_mock();
        let with_thinking = make_mock();
        let mut caps = caliban_provider::Capabilities {
            max_input_tokens: 100_000,
            max_output_tokens: 4096,
            vision: false,
            tool_use: caliban_provider::ToolUseCapability::Basic,
            thinking: true,
            prompt_caching: caliban_provider::PromptCachingCapability::None,
            json_mode: false,
            streaming: true,
            stop_sequences: true,
            top_k: false,
            system_prompt: caliban_provider::SystemPromptCapability::SeparateField,
            refusal_field: false,
        };
        with_thinking.set_capabilities(caps);
        caps.thinking = false;
        no_thinking.set_capabilities(caps);
        with_thinking.enqueue_complete(Ok(ok_response("think")));
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("nt", no_thinking)
            .add_provider("t", with_thinking)
            .route(RequestPurpose::MainLoop, "nt", "no-think")
            .route(RequestPurpose::MainLoop, "t", "with-think")
            .build()
            .unwrap();
        let mut req = req_main_loop();
        req.thinking = Some(caliban_provider::ThinkingConfig {
            budget_tokens: 4096,
        });
        let resp = r.complete(req).await.unwrap();
        assert_eq!(resp.model, "think");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_cache_markers_stripped_on_cross_route_hop() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::ServerError {
            status: 503,
            body: "down".into(),
        }));
        let secondary = make_mock();
        secondary.enqueue_complete(Ok(ok_response("ok")));
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", primary)
            .add_provider("b", secondary.clone())
            .route(RequestPurpose::MainLoop, "a", "x")
            .route(RequestPurpose::MainLoop, "b", "y")
            .build()
            .unwrap();
        let mut req = req_main_loop();
        req.messages = vec![caliban_provider::Message {
            role: Role::User,
            content: vec![caliban_provider::ContentBlock::Text(
                caliban_provider::TextBlock {
                    text: "hi".into(),
                    cache_control: Some(caliban_provider::CacheControl::Ephemeral),
                },
            )],
        }];
        let _ = r.complete(req).await.unwrap();
        let stats = r.stats();
        // The secondary call's request had its cache marker stripped.
        assert!(stats.cache_markers_cleared >= 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fallback_exhausted_returns_useful_error_listing_tried_routes() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::ServerError {
            status: 500,
            body: "x".into(),
        }));
        let secondary = make_mock();
        secondary.enqueue_complete(Err(ProviderError::ServerError {
            status: 500,
            body: "y".into(),
        }));
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", primary)
            .add_provider("b", secondary)
            .route(RequestPurpose::MainLoop, "a", "x")
            .route(RequestPurpose::MainLoop, "b", "y")
            .build()
            .unwrap();
        let err = r.complete(req_main_loop()).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("a:x:main_loop"), "msg: {msg}");
        assert!(msg.contains("b:y:main_loop"), "msg: {msg}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn explicit_fallback_ids_override_declaration_order() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::ModelUnavailable("nope".into())));
        let middle = make_mock();
        // middle should NOT be hit — explicit fallback skips it.
        let target = make_mock();
        target.enqueue_complete(Ok(ok_response("target")));
        let mut routes_cfg = vec![
            RouteEntry {
                id: "primary".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "a".into(),
                model: "x".into(),
                requires: CapabilityRequirements::default(),
                fallback: Some(vec!["target".into()]),
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
            RouteEntry {
                id: "middle".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "b".into(),
                model: "y".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
            RouteEntry {
                id: "target".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "c".into(),
                model: "z".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
        ];
        routes_cfg.shrink_to_fit();
        let mut providers: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
        providers.insert("a".into(), primary);
        providers.insert("b".into(), middle);
        providers.insert("c".into(), target);
        let r = ModelRouter::from_config(
            RouterConfig {
                default_purpose: RequestPurpose::MainLoop,
                routes: routes_cfg,
            },
            providers,
        )
        .unwrap();
        let resp = r.complete(req_main_loop()).await.unwrap();
        assert_eq!(resp.model, "target");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_fallback_array_disables_implicit_chain() {
        let primary = make_mock();
        primary.enqueue_complete(Err(ProviderError::ModelUnavailable("nope".into())));
        let target = make_mock();
        // Should never be called.
        target.enqueue_complete(Ok(ok_response("target")));
        let routes_cfg = vec![
            RouteEntry {
                id: "primary".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "a".into(),
                model: "x".into(),
                requires: CapabilityRequirements::default(),
                fallback: Some(vec![]),
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
            RouteEntry {
                id: "target".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "b".into(),
                model: "y".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
        ];
        let mut providers: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
        providers.insert("a".into(), primary);
        providers.insert("b".into(), target);
        let r = ModelRouter::from_config(
            RouterConfig {
                default_purpose: RequestPurpose::MainLoop,
                routes: routes_cfg,
            },
            providers,
        )
        .unwrap();
        let err = r.complete(req_main_loop()).await.unwrap_err();
        // With explicit empty fallback, the first failure should be the only error.
        assert!(matches!(err, ProviderError::ModelUnavailable(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn breaker_blocks_route_when_tripped() {
        let primary = make_mock();
        for _ in 0..3 {
            primary.enqueue_complete(Err(ProviderError::ServerError {
                status: 500,
                body: "x".into(),
            }));
        }
        let secondary = make_mock();
        for _ in 0..3 {
            secondary.enqueue_complete(Ok(ok_response("ok")));
        }
        let routes_cfg = vec![
            RouteEntry {
                id: "primary".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "a".into(),
                model: "x".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy {
                    failure_threshold: 1,
                    window: std::time::Duration::from_secs(60),
                    cooldown: std::time::Duration::from_secs(60),
                    half_open_probes: 1,
                },
                effort: None,
                effort_map: EffortMap::default(),
            },
            RouteEntry {
                id: "secondary".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "b".into(),
                model: "y".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
        ];
        let mut providers: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
        providers.insert("a".into(), primary);
        providers.insert("b".into(), secondary);
        let r = ModelRouter::from_config(
            RouterConfig {
                default_purpose: RequestPurpose::MainLoop,
                routes: routes_cfg,
            },
            providers,
        )
        .unwrap();
        // First call: primary fails, fallback to secondary (success), primary
        // breaker trips.
        let _ = r.complete(req_main_loop()).await.unwrap();
        assert!(matches!(
            r.breaker("primary").unwrap().state(),
            BreakerState::Tripped { .. }
        ));
        // Second call: primary tripped → resolver skips it; secondary directly.
        let cands = r.resolve(&req_main_loop()).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(r.routes[cands[0].route_idx].id, "secondary");
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn hedge_fires_after_configured_delay_and_winner_returns() {
        let primary = make_mock();
        // primary takes a long time before responding; the hedge target wins.
        primary.enqueue_complete(Ok(ok_response("primary")));
        let secondary = make_mock();
        secondary.enqueue_complete(Ok(ok_response("hedge")));

        let routes_cfg = vec![
            RouteEntry {
                id: "primary".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "a".into(),
                model: "x".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Race {
                    hedge_after: std::time::Duration::from_millis(50),
                    max_hedges: 1,
                },
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
            RouteEntry {
                id: "secondary".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "b".into(),
                model: "y".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
        ];
        let mut providers: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
        // Wrap the mocks in a delayed provider for primary.
        providers.insert(
            "a".into(),
            Arc::new(SlowProvider {
                inner: primary,
                delay: std::time::Duration::from_secs(60),
            }),
        );
        providers.insert("b".into(), secondary);
        let r = ModelRouter::from_config(
            RouterConfig {
                default_purpose: RequestPurpose::MainLoop,
                routes: routes_cfg,
            },
            providers,
        )
        .unwrap();
        let resp = r.complete(req_main_loop()).await.unwrap();
        // The hedge target wins.
        assert_eq!(resp.model, "hedge");
        let stats = r.stats();
        assert_eq!(stats.per_route["secondary"].hedge_wins, 1);
    }

    /// Wraps a provider to inject a delay before its `complete` resolves.
    struct SlowProvider {
        inner: Arc<MockProvider>,
        delay: std::time::Duration,
    }
    #[async_trait]
    impl Provider for SlowProvider {
        async fn complete(&self, req: CompletionRequest) -> ProviderResult<CompletionResponse> {
            tokio::time::sleep(self.delay).await;
            self.inner.complete(req).await
        }
        async fn stream(&self, req: CompletionRequest) -> ProviderResult<MessageStream> {
            tokio::time::sleep(self.delay).await;
            self.inner.stream(req).await
        }
        fn capabilities(&self, m: &str) -> Capabilities {
            self.inner.capabilities(m)
        }
        fn list_models(&self) -> Vec<ModelInfo> {
            self.inner.list_models()
        }
        fn name(&self) -> &'static str {
            "slow"
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn hedge_target_unreachable_original_race_still_works() {
        // The hedge target errors immediately; the primary's slow response
        // eventually wins.
        let primary = make_mock();
        primary.enqueue_complete(Ok(ok_response("primary")));
        let hedge_target = make_mock();
        hedge_target.enqueue_complete(Err(ProviderError::ServerError {
            status: 503,
            body: "down".into(),
        }));

        let routes_cfg = vec![
            RouteEntry {
                id: "primary".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "a".into(),
                model: "x".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Race {
                    hedge_after: std::time::Duration::from_millis(10),
                    max_hedges: 1,
                },
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
            RouteEntry {
                id: "hedge".into(),
                purpose: RequestPurpose::MainLoop,
                provider: "b".into(),
                model: "y".into(),
                requires: CapabilityRequirements::default(),
                fallback: None,
                hedge: HedgePolicy::Disabled,
                breaker: BreakerPolicy::disabled(),
                effort: None,
                effort_map: EffortMap::default(),
            },
        ];
        let mut providers: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
        providers.insert(
            "a".into(),
            Arc::new(SlowProvider {
                inner: primary,
                delay: std::time::Duration::from_millis(200),
            }),
        );
        providers.insert("b".into(), hedge_target);
        let r = ModelRouter::from_config(
            RouterConfig {
                default_purpose: RequestPurpose::MainLoop,
                routes: routes_cfg,
            },
            providers,
        )
        .unwrap();
        let resp = r.complete(req_main_loop()).await.unwrap();
        assert_eq!(resp.model, "primary");
    }

    #[test]
    fn diagnostics_renders_kept_and_dropped_routes() {
        let r = ModelRouter::builder()
            .default_purpose(RequestPurpose::MainLoop)
            .add_provider("a", make_mock())
            .add_provider("b", make_mock())
            .route(RequestPurpose::MainLoop, "a", "primary")
            .route(RequestPurpose::Summarization, "b", "haiku")
            .build()
            .unwrap();
        let (_cands, diag) = r.resolve_diagnostics(&req_main_loop()).unwrap();
        let out = render_diagnostics(RequestPurpose::MainLoop, DerivedNeeds::default(), &diag);
        assert!(out.contains("a:primary:main_loop"));
        assert!(out.contains("b:haiku:summarization"));
    }

    #[test]
    fn unknown_fallback_id_fails_at_build() {
        let mut providers: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
        providers.insert("a".into(), make_mock());
        let routes = vec![RouteEntry {
            id: "primary".into(),
            purpose: RequestPurpose::MainLoop,
            provider: "a".into(),
            model: "x".into(),
            requires: CapabilityRequirements::default(),
            fallback: Some(vec!["missing".into()]),
            hedge: HedgePolicy::Disabled,
            breaker: BreakerPolicy::disabled(),
            effort: None,
            effort_map: EffortMap::default(),
        }];
        let err = ModelRouter::from_config(
            RouterConfig {
                default_purpose: RequestPurpose::MainLoop,
                routes,
            },
            providers,
        )
        .unwrap_err();
        assert!(matches!(err, RouterError::UnknownFallbackId { .. }));
    }
}
