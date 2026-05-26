//! Integration tests for `Tool::parallel_conflict_key` (ADR 0016 Revised
//! 2026-05-26). Verifies the dispatcher groups batched calls by key — same
//! key serializes, different keys (and `None` keys) parallelize.
//!
//! Every test uses `MockProvider` to script a turn with multiple `tool_use`
//! blocks. Test tools sleep for fixed durations so wall-clock measurements
//! distinguish serial from parallel execution.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, HookDecision, Hooks, Message, Result as AgentResult, TextBlock, Tool,
    ToolContext, ToolCtx, ToolError, ToolRegistry, TurnEvent,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Test tools
// ---------------------------------------------------------------------------

/// Sleeps, then returns a single text block. The conflict key is read directly
/// from the JSON input field named "key" — tests put the same value in two
/// different tool calls to assert serialization, or distinct values for
/// parallelism.
struct KeyedSleepyTool {
    name: String,
    delay: Duration,
    schema: serde_json::Value,
}

impl KeyedSleepyTool {
    fn new(name: &str, delay: Duration) -> Self {
        Self {
            name: name.to_string(),
            delay,
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for KeyedSleepyTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &'static str {
        "keyed sleepy test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    fn parallel_conflict_key(&self, input: &serde_json::Value) -> Option<String> {
        input
            .get("key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
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

/// Tool with no `parallel_conflict_key` override — always parallel-safe.
struct PlainSleepyTool {
    name: String,
    delay: Duration,
    schema: serde_json::Value,
}

impl PlainSleepyTool {
    fn new(name: &str, delay: Duration) -> Self {
        Self {
            name: name.to_string(),
            delay,
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

#[async_trait]
impl Tool for PlainSleepyTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &'static str {
        "plain sleepy test tool"
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

struct AllowAllHooks;

#[async_trait]
impl Hooks for AllowAllHooks {
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> AgentResult<HookDecision> {
        Ok(HookDecision::Allow)
    }
}

// ---------------------------------------------------------------------------
// Mock-provider scripting helpers
// ---------------------------------------------------------------------------

/// Stream events for an assistant turn that emits `tool_use` blocks for each
/// `(tool_use_id, tool_name, input_json)` triple.
fn tool_turn_with_inputs(
    tools: &[(&str, &str, &str)],
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    let mut events = Vec::new();
    events.push(Ok(StreamEvent::MessageStart {
        id: "msg_par".into(),
        model: "mock-model".into(),
    }));
    for (i, (id, name, input_json)) in tools.iter().enumerate() {
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
            delta: StreamingDelta::ToolUseInputJson((*input_json).to_string()),
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

async fn drive_until_done(agent: Arc<Agent>) -> usize {
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("go")], CancellationToken::new());
    let mut tool_call_ends = 0;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::ToolCallEnd { .. } = ev.unwrap() {
            tool_call_ends += 1;
        }
    }
    tool_call_ends
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

const DISPATCH_DELAY: Duration = Duration::from_millis(200);

/// Two calls with DIFFERENT keys must parallelize (~200 ms total, not 400).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distinct_keys_parallelize() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(tool_turn_with_inputs(&[
        ("t1", "keyed", r#"{"key":"a"}"#),
        ("t2", "keyed", r#"{"key":"b"}"#),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(KeyedSleepyTool::new("keyed", DISPATCH_DELAY)));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .hooks(Arc::new(AllowAllHooks))
        .parallel_tool_limit(NonZeroUsize::new(4).unwrap())
        .build()
        .unwrap();

    let start = Instant::now();
    let n = drive_until_done(Arc::new(agent)).await;
    let elapsed = start.elapsed();

    assert_eq!(n, 2);
    assert!(
        elapsed < Duration::from_millis(350),
        "expected parallel (~200 ms), took {elapsed:?}"
    );
}

/// Two calls with the SAME key must serialize (~400 ms total).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_key_serializes() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(tool_turn_with_inputs(&[
        ("t1", "keyed", r#"{"key":"shared"}"#),
        ("t2", "keyed", r#"{"key":"shared"}"#),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(KeyedSleepyTool::new("keyed", DISPATCH_DELAY)));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .hooks(Arc::new(AllowAllHooks))
        .parallel_tool_limit(NonZeroUsize::new(4).unwrap())
        .build()
        .unwrap();

    let start = Instant::now();
    let n = drive_until_done(Arc::new(agent)).await;
    let elapsed = start.elapsed();

    assert_eq!(n, 2);
    assert!(
        elapsed >= Duration::from_millis(380),
        "expected serial (~400 ms), took {elapsed:?}"
    );
}

/// A keyed call paired with a plain (no-key) call on the same logical target
/// must parallelize — the plain call has no key, so it lands in the `None`
/// group and runs concurrently with the keyed one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyed_plus_plain_parallelize() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(tool_turn_with_inputs(&[
        ("t1", "keyed", r#"{"key":"shared"}"#),
        ("t2", "plain", r#"{"key":"shared"}"#),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(KeyedSleepyTool::new("keyed", DISPATCH_DELAY)));
    registry.register(Arc::new(PlainSleepyTool::new("plain", DISPATCH_DELAY)));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .hooks(Arc::new(AllowAllHooks))
        .parallel_tool_limit(NonZeroUsize::new(4).unwrap())
        .build()
        .unwrap();

    let start = Instant::now();
    let n = drive_until_done(Arc::new(agent)).await;
    let elapsed = start.elapsed();

    assert_eq!(n, 2);
    assert!(
        elapsed < Duration::from_millis(350),
        "expected parallel (~200 ms), took {elapsed:?}"
    );
}

/// Three calls: two share a key (serialize), one is independent — total wall
/// should be ~400 ms (the serial pair dominates).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_key_serializes_alongside_independent_call() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(tool_turn_with_inputs(&[
        ("t1", "keyed", r#"{"key":"x"}"#),
        ("t2", "keyed", r#"{"key":"x"}"#),
        ("t3", "keyed", r#"{"key":"y"}"#),
    ]));
    mp.enqueue_stream(end_turn_events());

    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(KeyedSleepyTool::new("keyed", DISPATCH_DELAY)));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock-model")
        .max_tokens(64)
        .hooks(Arc::new(AllowAllHooks))
        .parallel_tool_limit(NonZeroUsize::new(4).unwrap())
        .build()
        .unwrap();

    let start = Instant::now();
    let n = drive_until_done(Arc::new(agent)).await;
    let elapsed = start.elapsed();

    assert_eq!(n, 3);
    assert!(
        elapsed >= Duration::from_millis(380) && elapsed < Duration::from_millis(550),
        "expected ~400 ms (serial pair, independent in parallel), took {elapsed:?}"
    );
}
