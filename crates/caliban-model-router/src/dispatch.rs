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

#[cfg(test)]
mod tests {
    //! Dispatch-loop tests focused on the paths the crate-level `tests`
    //! module doesn't exercise: `rewrite_for_route` directly and the
    //! `dispatch_stream` fallback/breaker/stats loop. Hedging is intentionally
    //! disabled here — the hedge race is covered by the timer-driven tests in
    //! `crate::tests` and isn't reachable from `dispatch_stream` at all.
    use std::collections::HashMap;
    use std::sync::Arc;

    use caliban_provider::{
        CacheControl, CompletionRequest, ContentBlock, Error as ProviderError, ImageBlock,
        ImageSource, Message, MockProvider, Provider, RequestPurpose, Role, StreamEvent, TextBlock,
        Tool,
    };
    use serde_json::json;

    use crate::ModelRouter;
    use crate::config::{
        BreakerPolicy, CapabilityRequirements, EffortMap, HedgePolicy, RouteEntry, RouterConfig,
    };

    // --- fixtures -----------------------------------------------------------

    fn mock() -> Arc<MockProvider> {
        Arc::new(MockProvider::new())
    }

    /// A `MainLoop` text request with no cache markers.
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
            thinking: caliban_provider::ThinkingSetting::Auto,
            effort: None,
            metadata: Default::default(),
        };
        r.metadata.purpose = Some(RequestPurpose::MainLoop);
        r
    }

    /// A request carrying an `Ephemeral` cache marker on its only text block
    /// plus a cache-marked tool, used to exercise cross-route stripping.
    fn req_with_markers() -> CompletionRequest {
        let mut r = req_main_loop();
        r.messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text(TextBlock {
                text: "hi".into(),
                cache_control: Some(CacheControl::Ephemeral),
            })],
        }];
        r.tools = vec![Tool {
            name: "T".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
            cache_control: Some(CacheControl::Ephemeral),
        }];
        r
    }

    /// A minimal but complete streaming response.
    fn stream_events() -> Vec<caliban_provider::error::Result<StreamEvent>> {
        vec![
            Ok(StreamEvent::MessageStart {
                id: "m".into(),
                model: "mock".into(),
            }),
            Ok(StreamEvent::MessageStop),
        ]
    }

    fn route(id: &str, provider: &str, model: &str) -> RouteEntry {
        RouteEntry {
            id: id.into(),
            purpose: RequestPurpose::MainLoop,
            provider: provider.into(),
            model: model.into(),
            requires: CapabilityRequirements::default(),
            fallback: None,
            hedge: HedgePolicy::Disabled,
            breaker: BreakerPolicy::disabled(),
            effort: None,
            effort_map: EffortMap::default(),
        }
    }

    fn router_from(
        routes: Vec<RouteEntry>,
        providers: Vec<(&str, Arc<dyn Provider + Send + Sync>)>,
    ) -> ModelRouter {
        let map: HashMap<String, Arc<dyn Provider + Send + Sync>> = providers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        ModelRouter::from_config(
            RouterConfig {
                default_purpose: RequestPurpose::MainLoop,
                routes,
            },
            map,
        )
        .unwrap()
    }

    // --- rewrite_for_route --------------------------------------------------

    #[test]
    fn rewrite_swaps_model_and_keeps_markers_when_not_cross_route() {
        let r = router_from(vec![route("primary", "a", "claude")], vec![("a", mock())]);
        let base = req_with_markers();
        let route = &r.routes[0];
        let out = r.rewrite_for_route(&base, route, false);
        // Model is swapped to the route's model.
        assert_eq!(out.model, "claude");
        // Not cross-route → markers retained.
        match &out.messages[0].content[0] {
            ContentBlock::Text(t) => assert!(t.cache_control.is_some()),
            other => panic!("expected text block, got {other:?}"),
        }
        assert!(out.tools[0].cache_control.is_some());
        // No cross-route hop → no markers counted as cleared.
        assert_eq!(r.stats().cache_markers_cleared, 0);
    }

    #[test]
    fn rewrite_strips_markers_and_records_stats_on_cross_route() {
        let r = router_from(vec![route("primary", "a", "claude")], vec![("a", mock())]);
        let base = req_with_markers();
        let route = &r.routes[0];
        let out = r.rewrite_for_route(&base, route, true);
        // Cross-route → text + tool markers stripped.
        match &out.messages[0].content[0] {
            ContentBlock::Text(t) => assert!(t.cache_control.is_none()),
            other => panic!("expected text block, got {other:?}"),
        }
        assert!(out.tools[0].cache_control.is_none());
        // Two markers (one text, one tool) recorded as cleared.
        assert_eq!(r.stats().cache_markers_cleared, 2);
        // The base request is untouched (rewrite clones).
        match &base.messages[0].content[0] {
            ContentBlock::Text(t) => assert!(t.cache_control.is_some()),
            other => panic!("expected text block, got {other:?}"),
        }
    }

    #[test]
    fn rewrite_cross_route_with_no_markers_does_not_touch_stats() {
        let r = router_from(vec![route("primary", "a", "claude")], vec![("a", mock())]);
        let base = req_main_loop();
        let route = &r.routes[0];
        let out = r.rewrite_for_route(&base, route, true);
        assert_eq!(out.model, "claude");
        // No markers present → stripping records nothing.
        assert_eq!(r.stats().cache_markers_cleared, 0);
    }

    // --- dispatch_stream ----------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn stream_success_records_call_and_breaker_success() {
        let p = mock();
        p.enqueue_stream(stream_events());
        let r = router_from(vec![route("primary", "a", "x")], vec![("a", p)]);
        let s = r.dispatch_stream(req_main_loop()).await;
        assert!(s.is_ok(), "expected stream ok");
        let stats = r.stats();
        // Stream success records a call with default (zero) usage.
        assert_eq!(stats.per_route["primary"].call_count, 1);
        assert_eq!(stats.per_route["primary"].input_tokens, 0);
        assert_eq!(stats.per_route["primary"].failures, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_falls_back_on_fatal_error() {
        let primary = mock();
        primary.enqueue_stream_error(ProviderError::ServerError {
            status: 503,
            body: "down".into(),
        });
        let secondary = mock();
        secondary.enqueue_stream(stream_events());
        let r = router_from(
            vec![route("primary", "a", "x"), route("secondary", "b", "y")],
            vec![("a", primary), ("b", secondary)],
        );
        let s = r.dispatch_stream(req_main_loop()).await;
        assert!(s.is_ok(), "expected fallback to succeed");
        let stats = r.stats();
        assert_eq!(stats.per_route["primary"].failures, 1);
        assert_eq!(stats.per_route["primary"].fallback_engaged, 1);
        assert_eq!(stats.per_route["secondary"].call_count, 1);
        // Both routes have breaker snapshots after the dispatch loop ran.
        assert!(stats.breakers.contains_key("primary"));
        assert!(stats.breakers.contains_key("secondary"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_does_not_fall_back_on_non_fatal_error() {
        let primary = mock();
        primary.enqueue_stream_error(ProviderError::Auth("bad".into()));
        // Secondary has nothing queued; if fallback engaged it would surface a
        // different (queue-empty) error.
        let secondary = mock();
        let r = router_from(
            vec![route("primary", "a", "x"), route("secondary", "b", "y")],
            vec![("a", primary), ("b", secondary)],
        );
        let err = match r.dispatch_stream(req_main_loop()).await {
            Ok(_) => panic!("expected non-fatal error to propagate"),
            Err(e) => e,
        };
        assert!(matches!(err, ProviderError::Auth(_)), "got {err:?}");
        let stats = r.stats();
        assert_eq!(stats.per_route["primary"].failures, 1);
        // No fallback engaged.
        assert_eq!(stats.per_route["primary"].fallback_engaged, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_exhausted_chain_returns_fallback_exhausted() {
        let primary = mock();
        primary.enqueue_stream_error(ProviderError::ModelUnavailable("a".into()));
        let secondary = mock();
        secondary.enqueue_stream_error(ProviderError::ModelUnavailable("b".into()));
        let r = router_from(
            vec![route("primary", "a", "x"), route("secondary", "b", "y")],
            vec![("a", primary), ("b", secondary)],
        );
        let err = match r.dispatch_stream(req_main_loop()).await {
            Ok(_) => panic!("expected exhausted chain to error"),
            Err(e) => e,
        };
        let msg = err.to_string();
        // Both tried route ids appear in the exhausted error.
        assert!(msg.contains("primary"), "msg: {msg}");
        assert!(msg.contains("secondary"), "msg: {msg}");
        let stats = r.stats();
        assert_eq!(stats.per_route["primary"].failures, 1);
        assert_eq!(stats.per_route["secondary"].failures, 1);
        // Fallback engaged once (primary → secondary); last route doesn't.
        assert_eq!(stats.per_route["primary"].fallback_engaged, 1);
        assert_eq!(stats.per_route["secondary"].fallback_engaged, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_strips_cache_markers_on_cross_route_hop() {
        let primary = mock();
        primary.enqueue_stream_error(ProviderError::ServerError {
            status: 503,
            body: "down".into(),
        });
        let secondary = mock();
        secondary.enqueue_stream(stream_events());
        let r = router_from(
            vec![route("primary", "a", "x"), route("secondary", "b", "y")],
            vec![("a", primary), ("b", secondary)],
        );
        let s = r.dispatch_stream(req_with_markers()).await;
        assert!(s.is_ok());
        // The cross-route hop to `secondary` stripped the text + tool markers.
        assert!(r.stats().cache_markers_cleared >= 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_empty_candidates_returns_no_candidate_error() {
        // The mock's default capabilities have vision=false, so a request
        // bearing an image resolves to an empty candidate list.
        let p = mock();
        let r = router_from(vec![route("primary", "a", "x")], vec![("a", p)]);
        let mut req = req_main_loop();
        req.messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source: ImageSource::Url {
                    url: "https://x/y.png".into(),
                },
                cache_control: None,
                sha256: None,
                dims: None,
            })],
        }];
        let err = match r.dispatch_stream(req).await {
            Ok(_) => panic!("expected no-candidate error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("no candidate"), "got: {err}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_default_usage_recorded_on_success() {
        // Distinguishes the stream success path: it records `Usage::default()`
        // (all zeros) regardless of what events the stream would carry.
        let p = mock();
        p.enqueue_stream(stream_events());
        let r = router_from(vec![route("primary", "a", "x")], vec![("a", p)]);
        let _ = r.dispatch_stream(req_main_loop()).await.unwrap();
        let stats = r.stats();
        let usage = &stats.per_route["primary"];
        assert_eq!(usage.call_count, 1);
        // Stream success records `Usage::default()` — all token counters zero.
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }
}
