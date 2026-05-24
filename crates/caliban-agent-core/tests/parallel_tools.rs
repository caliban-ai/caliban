//! Integration tests for parallel tool dispatch.
//!
//! Every test uses `MockProvider` to script a turn with multiple
//! `tool_use` blocks. Tools are crafted to expose timing, ordering, and
//! concurrency invariants.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, HookDecision, Hooks, Message, StopCondition, TextBlock, Tool, ToolContext,
    ToolCtx, ToolError, ToolRegistry, TurnEvent,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Test tools
// ---------------------------------------------------------------------------

/// A tool that sleeps for `delay`, then returns a single text block whose
/// content is the tool's name. Used to measure parallel vs serial wall time.
struct SleepyTool {
    name: String,
    delay: Duration,
    schema: serde_json::Value,
}

impl SleepyTool {
    fn new(name: &str, delay: Duration) -> Self {
        Self {
            name: name.to_string(),
            delay,
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for SleepyTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "sleepy test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        tokio::select! {
            () = tokio::time::sleep(self.delay) => {}
            () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
        }
        Ok(vec![ContentBlock::Text(TextBlock {
            text: self.name.clone(),
            cache_control: None,
        })])
    }
}

/// A tool that increments a shared `(current, peak)` counter on entry,
/// sleeps, then decrements `current`. Used to assert the semaphore cap.
struct TrackingTool {
    name: String,
    state: Arc<Mutex<(usize, usize)>>,
    delay: Duration,
    schema: serde_json::Value,
}

impl TrackingTool {
    fn new(name: &str, state: Arc<Mutex<(usize, usize)>>, delay: Duration) -> Self {
        Self {
            name: name.to_string(),
            state,
            delay,
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for TrackingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "tracking test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        {
            let mut s = self.state.lock().unwrap();
            s.0 += 1;
            s.1 = s.1.max(s.0);
        }
        tokio::time::sleep(self.delay).await;
        {
            let mut s = self.state.lock().unwrap();
            s.0 -= 1;
        }
        Ok(Vec::new())
    }
}

/// `Hooks` impl that denies tool calls whose name appears in `deny_names`.
struct DenyingHooks {
    deny_names: Vec<String>,
}

#[async_trait]
impl Hooks for DenyingHooks {
    async fn before_tool(
        &self,
        ctx: &ToolCtx<'_>,
    ) -> caliban_agent_core::Result<HookDecision> {
        if self.deny_names.iter().any(|n| n == ctx.tool_name) {
            Ok(HookDecision::Deny(format!("denied: {}", ctx.tool_name)))
        } else {
            Ok(HookDecision::Allow)
        }
    }
}

// ---------------------------------------------------------------------------
// Mock-provider scripting helpers
// ---------------------------------------------------------------------------

/// Stream events for an assistant turn that emits `tool_use` blocks for each
/// `(tool_use_id, name)` pair, all at distinct content-block indices, then
/// stops with `StopReason::ToolUse`.
fn parallel_tool_turn(
    tools: &[(&str, &str)],
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    let mut events = Vec::new();
    events.push(Ok(StreamEvent::MessageStart {
        id: "msg_par".into(),
        model: "mock-model".into(),
    }));
    for (i, (id, name)) in tools.iter().enumerate() {
        let idx = u32::try_from(i).unwrap();
        events.push(Ok(StreamEvent::ContentBlockStart {
            index: idx,
            content_type: StreamingContentType::ToolUse {
                id: (*id).to_string(),
                name: (*name).to_string(),
            },
        }));
        events.push(Ok(StreamEvent::Delta {
            index: idx,
            delta: StreamingDelta::ToolUseInputJson("{}".into()),
        }));
        events.push(Ok(StreamEvent::ContentBlockStop { index: idx }));
    }
    events.push(Ok(StreamEvent::MessageDelta {
        stop_reason: Some(StopReason::ToolUse),
        usage_delta: Some(Usage::default()),
    }));
    events.push(Ok(StreamEvent::MessageStop));
    events
}

/// Stream events for a turn that produces an `EndTurn` with no content.
/// Used to terminate the run after the tool-call turn.
fn end_turn_events() -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_end".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_tool_one_turn_still_works() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[("t1", "sleepy_a")]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(SleepyTool::new("sleepy_a", Duration::from_millis(5))));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .build()
        .unwrap();

    let mut stream =
        Arc::new(agent).stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    let mut tool_call_ends = 0;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::ToolCallEnd { .. } = ev.unwrap() {
            tool_call_ends += 1;
        }
    }
    assert_eq!(tool_call_ends, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_is_faster_than_serial() {
    fn build() -> (Arc<MockProvider>, ToolRegistry) {
        let mp = Arc::new(MockProvider::new());
        mp.enqueue_stream(parallel_tool_turn(&[
            ("t1", "sleepy_a"),
            ("t2", "sleepy_b"),
            ("t3", "sleepy_c"),
        ]));
        mp.enqueue_stream(end_turn_events());
        let mut registry = ToolRegistry::default();
        let d = Duration::from_millis(100);
        registry.register(Arc::new(SleepyTool::new("sleepy_a", d)));
        registry.register(Arc::new(SleepyTool::new("sleepy_b", d)));
        registry.register(Arc::new(SleepyTool::new("sleepy_c", d)));
        (mp, registry)
    }

    async fn run_with(parallel: bool) -> Duration {
        let (mp, registry) = build();
        let agent = Agent::builder()
            .provider(mp as Arc<dyn Provider + Send + Sync>)
            .tools(registry)
            .model("mock-model")
            .max_tokens(64)
            .parallel_tools(parallel)
            .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
            .build()
            .unwrap();
        let start = Instant::now();
        let mut s = Arc::new(agent).stream_until_done(
            vec![Message::user_text("hi")],
            CancellationToken::new(),
        );
        while let Some(ev) = s.next().await {
            ev.unwrap();
        }
        start.elapsed()
    }

    let parallel_wall = run_with(true).await;
    let serial_wall = run_with(false).await;

    assert!(
        parallel_wall < Duration::from_millis(200),
        "parallel wall {parallel_wall:?} should be < 200ms (3 × 100ms in parallel)"
    );
    assert!(
        serial_wall >= Duration::from_millis(280),
        "serial wall {serial_wall:?} should be >= 280ms (3 × 100ms serially)"
    );
    assert!(
        parallel_wall.as_millis() * 2 < serial_wall.as_millis(),
        "parallel {parallel_wall:?} should be at least 2× faster than serial {serial_wall:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn history_in_assistant_order_events_in_completion_order() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("ta", "a"),
        ("tb", "b"),
        ("tc", "c"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(SleepyTool::new("a", Duration::from_millis(80))));
    registry.register(Arc::new(SleepyTool::new("b", Duration::from_millis(40))));
    registry.register(Arc::new(SleepyTool::new("c", Duration::from_millis(5))));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
        .build()
        .unwrap();

    let mut stream =
        Arc::new(agent).stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());

    let mut event_order: Vec<String> = Vec::new();
    let mut final_messages: Vec<Message> = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            TurnEvent::ToolCallEnd { tool_use_id, .. } => event_order.push(tool_use_id),
            TurnEvent::RunEnd {
                final_messages: fm, ..
            } => final_messages = fm,
            _ => {}
        }
    }

    assert_eq!(
        event_order,
        vec!["tc", "tb", "ta"],
        "ToolCallEnd must arrive in completion order"
    );

    let tool_results_msg = final_messages
        .iter()
        .find(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult(_)))
        })
        .expect("tool-results message present in history");
    let history_ids: Vec<&str> = tool_results_msg
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult(tr) => Some(tr.tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        history_ids,
        vec!["ta", "tb", "tc"],
        "history must be in assistant-message order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denied_tool_keeps_its_history_slot() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("ta", "a"),
        ("tb", "b"),
        ("tc", "c"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(SleepyTool::new("a", Duration::from_millis(20))));
    registry.register(Arc::new(SleepyTool::new("b", Duration::from_millis(20))));
    registry.register(Arc::new(SleepyTool::new("c", Duration::from_millis(20))));

    let hooks = Arc::new(DenyingHooks {
        deny_names: vec!["b".to_string()],
    });

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .hooks(hooks)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
        .build()
        .unwrap();

    let mut stream =
        Arc::new(agent).stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());

    let mut final_messages: Vec<Message> = Vec::new();
    let mut denied_seen = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            TurnEvent::ToolCallEnd {
                tool_use_id,
                is_error,
                content,
                ..
            } if tool_use_id == "tb" => {
                denied_seen = true;
                assert!(is_error, "denied tool's ToolCallEnd must have is_error=true");
                let text = match &content[0] {
                    ContentBlock::Text(t) => t.text.clone(),
                    _ => panic!("expected text block in denial"),
                };
                assert!(
                    text.contains("denied"),
                    "denial content should mention denial; got {text:?}"
                );
            }
            TurnEvent::RunEnd {
                final_messages: fm, ..
            } => final_messages = fm,
            _ => {}
        }
    }
    assert!(denied_seen);

    let tool_results_msg = final_messages
        .iter()
        .find(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult(_)))
        })
        .expect("tool-results message present");
    let history_ids: Vec<&str> = tool_results_msg
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult(tr) => Some(tr.tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        history_ids,
        vec!["ta", "tb", "tc"],
        "denied tool must keep its slot in assistant-message order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_drains_in_flight() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("t1", "long_a"),
        ("t2", "long_b"),
        ("t3", "long_c"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    let d = Duration::from_millis(200);
    registry.register(Arc::new(SleepyTool::new("long_a", d)));
    registry.register(Arc::new(SleepyTool::new("long_b", d)));
    registry.register(Arc::new(SleepyTool::new("long_c", d)));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(3).unwrap())
        .build()
        .unwrap();

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });

    let start = Instant::now();
    let mut s = Arc::new(agent).stream_until_done(vec![Message::user_text("hi")], cancel);
    let mut stop_condition: Option<StopCondition> = None;
    while let Some(ev) = s.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev.unwrap() {
            stop_condition = Some(stopped_for);
        }
    }
    let elapsed = start.elapsed();

    assert!(
        matches!(stop_condition, Some(StopCondition::Cancelled)),
        "run must terminate with StopCondition::Cancelled; got {stop_condition:?}"
    );
    assert!(
        elapsed < Duration::from_millis(150),
        "cancellation should propagate quickly; elapsed = {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn semaphore_limit_caps_concurrency() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("t1", "track_1"),
        ("t2", "track_2"),
        ("t3", "track_3"),
        ("t4", "track_4"),
        ("t5", "track_5"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let state = Arc::new(Mutex::new((0_usize, 0_usize)));
    let mut registry = ToolRegistry::default();
    let d = Duration::from_millis(40);
    for i in 1..=5 {
        registry.register(Arc::new(TrackingTool::new(
            &format!("track_{i}"),
            Arc::clone(&state),
            d,
        )));
    }

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(true)
        .parallel_tool_limit(NonZeroUsize::new(2).unwrap())
        .build()
        .unwrap();

    let mut s = Arc::new(agent).stream_until_done(
        vec![Message::user_text("hi")],
        CancellationToken::new(),
    );
    while let Some(ev) = s.next().await {
        ev.unwrap();
    }

    let peak = state.lock().unwrap().1;
    assert_eq!(peak, 2, "with limit=2, peak concurrent must be exactly 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_tools_false_is_serial() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(parallel_tool_turn(&[
        ("t1", "track_1"),
        ("t2", "track_2"),
        ("t3", "track_3"),
    ]));
    mp.enqueue_stream(end_turn_events());

    let state = Arc::new(Mutex::new((0_usize, 0_usize)));
    let mut registry = ToolRegistry::default();
    let d = Duration::from_millis(30);
    for i in 1..=3 {
        registry.register(Arc::new(TrackingTool::new(
            &format!("track_{i}"),
            Arc::clone(&state),
            d,
        )));
    }

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .parallel_tools(false)
        .parallel_tool_limit(NonZeroUsize::new(8).unwrap())
        .build()
        .unwrap();

    let mut s = Arc::new(agent).stream_until_done(
        vec![Message::user_text("hi")],
        CancellationToken::new(),
    );
    let mut event_order: Vec<String> = Vec::new();
    while let Some(ev) = s.next().await {
        if let TurnEvent::ToolCallEnd { tool_use_id, .. } = ev.unwrap() {
            event_order.push(tool_use_id);
        }
    }

    let peak = state.lock().unwrap().1;
    assert_eq!(peak, 1, "with parallel_tools=false, peak concurrent must be 1");
    assert_eq!(event_order, vec!["t1", "t2", "t3"]);
}
