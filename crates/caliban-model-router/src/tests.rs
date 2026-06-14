//! Router-level tests: covers builder validation, resolution ordering,
//! fallback/hedge dispatch, breaker behavior, and diagnostic rendering.

use super::*;
use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, Error as ProviderError, Message,
    MessageStream, MockProvider, ModelInfo, Role, StopReason, Usage,
    error::Result as ProviderResult,
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
        thinking: caliban_provider::ThinkingSetting::Auto,
        effort: None,
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
    req.thinking = caliban_provider::ThinkingSetting::On(Some(4096));
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
