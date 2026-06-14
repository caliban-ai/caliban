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
//! `docs/adr/0038-model-router-v2.md`.

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
mod builder;
pub mod cache;
pub mod capabilities;
pub mod config;
pub mod discovery;
mod dispatch;
pub mod effort;
pub mod error;
pub mod fallback;
pub mod hedging;
mod provider_impl;
pub mod resolver;

#[cfg(test)]
mod tests;

pub use breaker::{BreakerSnapshot, BreakerState, CircuitBreaker};
pub use builder::ModelRouterBuilder;
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

use caliban_provider::{CompletionRequest, Provider, RequestPurpose};

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
pub(crate) struct StatsInner {
    pub(crate) per_route: HashMap<String, RouteUsage>,
    pub(crate) cache_markers_cleared: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct StatsHandle(pub(crate) Arc<Mutex<StatsInner>>);

impl StatsHandle {
    pub(crate) fn record_success(&self, route_id: &str, usage: caliban_provider::Usage) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.call_count += 1;
        entry.input_tokens += u64::from(usage.input_tokens);
        entry.output_tokens += u64::from(usage.output_tokens);
        entry.cache_read_input_tokens += u64::from(usage.cache_read_input_tokens.unwrap_or(0));
        entry.cache_creation_input_tokens +=
            u64::from(usage.cache_creation_input_tokens.unwrap_or(0));
    }

    pub(crate) fn record_failure(&self, route_id: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.failures += 1;
    }

    pub(crate) fn record_hedge_loss(&self, route_id: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.hedge_losses += 1;
    }

    pub(crate) fn record_hedge_win(&self, route_id: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(route_id.to_string()).or_default();
        entry.hedge_wins += 1;
    }

    pub(crate) fn record_fallback_engaged(&self, from: &str) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        let entry = guard.per_route.entry(from.to_string()).or_default();
        entry.fallback_engaged += 1;
    }

    pub(crate) fn record_cache_markers_cleared(&self, n: u32) {
        let mut guard = self.0.lock().expect("router stats lock poisoned");
        guard.cache_markers_cleared += u64::from(n);
    }

    pub(crate) fn snapshot(
        &self,
        breakers: &HashMap<String, CircuitBreaker>,
    ) -> RouterStatsSnapshot {
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
    pub(crate) default_purpose: RequestPurpose,
    pub(crate) routes: Vec<RouteEntry>,
    pub(crate) providers: Arc<HashMap<String, Arc<dyn Provider + Send + Sync>>>,
    pub(crate) breakers: Arc<HashMap<String, CircuitBreaker>>,
    pub(crate) stats: StatsHandle,
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

    pub(crate) fn new(
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
