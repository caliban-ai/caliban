//! Integration tests for the per-tool-call dispatch record (#256).
//!
//! The `--debug-file` log must contain one structured, greppable record per
//! *tool call* — not just the per-turn aggregate — so a reader (human or
//! agent) can reconstruct the exact per-turn tool sequence and counts. The
//! record fires for **every** tool regardless of whether the tool itself emits
//! internal tracing (the root cause of #256: only `Grep` was visible because
//! its `ignore`/`globset` file-walk emitted DEBUG lines under the dispatch
//! span; `Read`/`Edit`/`Bash`/etc. were invisible).
//!
//! Run with:
//!
//! ```text
//! cargo test -p caliban-agent-core --features caliban-provider/mock dispatch_events
//! ```

#![allow(missing_docs)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, AgentConfig, ContentBlock, Message, TextBlock, Tool, ToolContext, ToolError,
    ToolRegistry,
};
use caliban_common::tracing_targets::TARGET_TOOLS;
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use tokio_util::sync::CancellationToken;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt as _;

// ---------------------------------------------------------------------------
// Tracing capture: a Layer that records every event's target + fields so the
// test can assert the per-tool dispatch records exist with the right shape.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CapturedEvent {
    target: String,
    fields: HashMap<String, String>,
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

struct FieldVisitor<'a>(&'a mut HashMap<String, String>);

impl Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldVisitor(&mut fields));
        self.events.lock().unwrap().push(CapturedEvent {
            target: event.metadata().target().to_owned(),
            fields,
        });
    }
}

impl CaptureLayer {
    /// Per-tool-call *completion* records: target == `TARGET_TOOLS`, carrying a
    /// `tool_use_id` and a `status` (distinguishes them from the per-turn
    /// aggregate, which has neither).
    fn completion_records(&self) -> Vec<HashMap<String, String>> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.target == TARGET_TOOLS)
            .filter(|e| e.fields.contains_key("tool_use_id") && e.fields.contains_key("status"))
            .map(|e| e.fields.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Stream-event builders (mirrors no_edit_nudge.rs / integration.rs helpers).
// ---------------------------------------------------------------------------

fn text_stream(
    msg_id: &str,
    text: &str,
    stop: StopReason,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: "mock-model".to_owned(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(stop),
            usage_delta: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn tool_use_stream(
    msg_id: &str,
    tool_use_id: &str,
    tool_name: &str,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: "mock-model".to_owned(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: tool_use_id.to_owned(),
                name: tool_name.to_owned(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson("{}".to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(Usage {
                input_tokens: 10,
                output_tokens: 3,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

// ---------------------------------------------------------------------------
// Mock tools: a silent read-only tool (emits NO internal tracing — the #256
// blind spot), a silent side-effecting tool, and a failing tool.
// ---------------------------------------------------------------------------

struct SilentTool {
    name: &'static str,
}

#[async_trait]
impl Tool for SilentTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "A tool that emits no internal tracing"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object", "properties": {}}))
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "ok".to_owned(),
            cache_control: None,
        })])
    }
}

struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &'static str {
        "boom"
    }
    fn description(&self) -> &'static str {
        "A tool that always errors"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object", "properties": {}}))
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        Err(ToolError::invalid_input("kaboom"))
    }
}

fn build_agent(mock: Arc<MockProvider>, registry: ToolRegistry) -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .provider(mock as Arc<dyn Provider + Send + Sync>)
            .tools(registry)
            .config(AgentConfig {
                model: "mock-model".to_owned(),
                max_tokens: 1024,
                max_turns: 50,
                no_edit_nudge_threshold: 0,
                ..AgentConfig::default()
            })
            .build()
            .expect("agent should build"),
    )
}

// ---------------------------------------------------------------------------
// Test 1 — one greppable dispatch record per tool call, for silent tools.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn emits_one_dispatch_record_per_tool_call() {
    let capture = CaptureLayer::default();
    let _guard = tracing_subscriber::registry()
        .with(capture.clone())
        .set_default();

    let mock = Arc::new(MockProvider::new());
    // Two silent tool calls across two turns, then a clean end turn. Neither
    // tool emits internal tracing, so pre-#256 they leave NO trace in the log.
    mock.enqueue_stream(tool_use_stream("m0", "call0", "peek"));
    mock.enqueue_stream(tool_use_stream("m1", "call1", "run"));
    mock.enqueue_stream(text_stream("m_end", "done", StopReason::EndTurn));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SilentTool { name: "peek" }));
    registry.register(Arc::new(SilentTool { name: "run" }));
    let agent = build_agent(Arc::clone(&mock), registry);

    agent
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run should succeed");

    let records = capture.completion_records();
    assert_eq!(
        records.len(),
        2,
        "exactly one dispatch completion record per tool call; got {records:#?}"
    );

    let by_id: HashMap<&str, &HashMap<String, String>> = records
        .iter()
        .map(|r| (r["tool_use_id"].as_str(), r))
        .collect();

    let peek = by_id.get("call0").expect("record for call0 (peek)");
    assert_eq!(peek["tool"], "peek");
    assert_eq!(peek["status"], "ok");
    assert!(peek.contains_key("turn_index"), "record carries turn_index");
    assert!(
        peek.contains_key("duration_ms"),
        "record carries duration_ms"
    );

    let run = by_id.get("call1").expect("record for call1 (run)");
    assert_eq!(run["tool"], "run");
    assert_eq!(run["status"], "ok");
}

// ---------------------------------------------------------------------------
// Test 2 — the dispatch record reflects tool errors via the `status` field.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_record_marks_tool_errors() {
    let capture = CaptureLayer::default();
    let _guard = tracing_subscriber::registry()
        .with(capture.clone())
        .set_default();

    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_use_stream("m0", "call0", "boom"));
    mock.enqueue_stream(text_stream("m_end", "done", StopReason::EndTurn));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FailingTool));
    let agent = build_agent(Arc::clone(&mock), registry);

    agent
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run should succeed (tool error is surfaced, not fatal)");

    let records = capture.completion_records();
    assert_eq!(records.len(), 1, "one record for the single tool call");
    assert_eq!(records[0]["tool"], "boom");
    assert_eq!(
        records[0]["status"], "error",
        "a failing tool must be recorded with status=error"
    );
}
