//! Integration test: when `plan_mode` is true, non-allowlisted tools are
//! rejected at the dispatcher with a synthesized `ToolResult` — they never
//! reach the tool's `invoke`.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, Message, TextBlock, Tool, ToolContext, ToolError, ToolRegistry, TurnEvent,
    new_shared_plan_mode,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

struct RecordingTool {
    name: String,
    schema: serde_json::Value,
    called: Arc<AtomicBool>,
}

impl RecordingTool {
    fn new(name: &str, called: Arc<AtomicBool>) -> Self {
        Self {
            name: name.into(),
            schema: serde_json::json!({"type":"object"}),
            called,
        }
    }
}

#[async_trait]
impl Tool for RecordingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &'static str {
        "recording test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        self.called.store(true, Ordering::Relaxed);
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "invoked".into(),
            cache_control: None,
        })])
    }
}

fn tool_turn(id: &str, name: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "m".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: id.into(),
                name: name.into(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson("{}".into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn end_turn() -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "e".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutating_tool_blocked_in_plan_mode() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(tool_turn("t1", "Bash"));
    mp.enqueue_stream(end_turn());

    let plan_mode = new_shared_plan_mode();
    plan_mode.store(true, Ordering::Relaxed);

    let called = Arc::new(AtomicBool::new(false));
    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(RecordingTool::new("Bash", Arc::clone(&called))));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock")
        .max_tokens(64)
        .parallel_tool_limit(NonZeroUsize::new(1).unwrap())
        .plan_mode(plan_mode)
        .build()
        .unwrap();

    let mut s =
        Arc::new(agent).stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    let mut got_error = false;
    while let Some(ev) = s.next().await {
        if let TurnEvent::ToolCallEnd {
            is_error, content, ..
        } = ev.unwrap()
            && is_error
        {
            got_error = true;
            let text = match &content[0] {
                ContentBlock::Text(t) => t.text.clone(),
                _ => String::new(),
            };
            assert!(text.contains("not available in plan mode"));
        }
    }
    assert!(got_error, "should have emitted a plan-mode rejection");
    assert!(
        !called.load(Ordering::Relaxed),
        "Bash::invoke must NOT run when plan mode rejects"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_tool_allowed_in_plan_mode() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(tool_turn("t1", "Read"));
    mp.enqueue_stream(end_turn());

    let plan_mode = new_shared_plan_mode();
    plan_mode.store(true, Ordering::Relaxed);

    let called = Arc::new(AtomicBool::new(false));
    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(RecordingTool::new("Read", Arc::clone(&called))));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock")
        .max_tokens(64)
        .parallel_tool_limit(NonZeroUsize::new(1).unwrap())
        .plan_mode(plan_mode)
        .build()
        .unwrap();

    let mut s =
        Arc::new(agent).stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    while let Some(ev) = s.next().await {
        ev.unwrap();
    }
    assert!(called.load(Ordering::Relaxed), "Read should be invoked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_mode_off_lets_mutating_tools_run() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(tool_turn("t1", "Bash"));
    mp.enqueue_stream(end_turn());

    let plan_mode = new_shared_plan_mode();
    // default false — leave it.

    let called = Arc::new(AtomicBool::new(false));
    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(RecordingTool::new("Bash", Arc::clone(&called))));

    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(registry)
        .model("mock")
        .max_tokens(64)
        .parallel_tool_limit(NonZeroUsize::new(1).unwrap())
        .plan_mode(plan_mode)
        .build()
        .unwrap();

    let mut s =
        Arc::new(agent).stream_until_done(vec![Message::user_text("hi")], CancellationToken::new());
    while let Some(ev) = s.next().await {
        ev.unwrap();
    }
    assert!(called.load(Ordering::Relaxed));
}
