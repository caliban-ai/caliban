//! Candidate dispatch — fallback + hedge loops shared between the
//! `complete` and `stream` Provider methods.
//!
//! Resolution is in [`crate::resolver`]; this module owns the loop that
//! turns a resolved candidate list into an actual provider call, observing
//! breakers, recording stats, and advancing on fatal-for-route errors.

use std::sync::Arc;

use caliban_provider::{
    CompletionRequest, CompletionResponse, Error as ProviderError, MessageStream,
    error::Result as ProviderResult,
};

use crate::ModelRouter;
use crate::cache;
use crate::config::{HedgePolicy, RouteEntry};
use crate::error::RouterError;
use crate::fallback::is_fatal_for_route;
use crate::hedging;

impl ModelRouter {
    /// Prepare a per-route request: clones the inbound, swaps `model` for
    /// the route's, strips cache markers when crossing a route boundary.
    pub(crate) fn rewrite_for_route(
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

    /// Drive the candidate list to produce a `CompletionResponse`, applying
    /// per-route hedging and sequential fallback on fatal-for-route errors.
    pub(crate) async fn dispatch_complete(
        &self,
        request: CompletionRequest,
    ) -> ProviderResult<CompletionResponse> {
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

    /// Drive the candidate list to produce a `MessageStream`. Hedging is
    /// disabled for streams in v2 — streams are stateful and racing them
    /// complicates cancellation in ways we don't want to ship behind a
    /// feature flag.
    pub(crate) async fn dispatch_stream(
        &self,
        request: CompletionRequest,
    ) -> ProviderResult<MessageStream> {
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
}
