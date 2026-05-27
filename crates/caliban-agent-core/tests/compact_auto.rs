//! Autocompact threshold + failure-backoff tests.

#![allow(missing_docs)]

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, AgentConfig, Compactor, ContentBlock, Message, TextBlock, ToolRegistry,
};
use caliban_provider::{
    Capabilities, MockProvider, PromptCachingCapability, Provider, StopReason, StreamEvent,
    StreamingContentType, StreamingDelta, SystemPromptCapability, ToolUseCapability, Usage,
};
use futures::StreamExt as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fake_caps(max_input: u32) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: 4_096,
        vision: false,
        tool_use: ToolUseCapability::Basic,
        thinking: false,
        prompt_caching: PromptCachingCapability::None,
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: SystemPromptCapability::SeparateField,
        refusal_field: false,
    }
}

fn text_endturn_stream(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg1".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
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
    tool_use_id: &str,
    tool_name: &str,
    input_json: &str,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg1".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: tool_use_id.into(),
                name: tool_name.into(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson(input_json.into()),
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
// Recording compactor: counts calls, optionally fails.
// ---------------------------------------------------------------------------

struct RecordingCompactor {
    calls: Arc<AtomicU32>,
    fail: bool,
}

#[async_trait]
impl Compactor for RecordingCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        _caps: &Capabilities,
    ) -> caliban_agent_core::error::Result<Option<Vec<Message>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(caliban_agent_core::error::Error::Compaction(
                "compact failed".into(),
            ))
        } else {
            Ok(Some(vec![messages.last().cloned().unwrap_or_else(|| {
                Message {
                    role: caliban_provider::Role::User,
                    content: vec![ContentBlock::Text(TextBlock {
                        text: "compacted".into(),
                        cache_control: None,
                    })],
                }
            })]))
        }
    }

    fn strategy_name(&self) -> &'static str {
        "test"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn autocompact_fires_above_threshold() {
    // Build a long history so estimate_tokens crosses 50%.
    let mut history = vec![Message::user_text("hi")];
    let filler = "x".repeat(100_000);
    for _ in 0..5 {
        history.push(Message::user_text(&filler));
    }
    let mock = Arc::new(MockProvider::new());
    mock.set_capabilities(fake_caps(200_000));
    mock.enqueue_stream(text_endturn_stream("done"));
    let provider: Arc<dyn Provider + Send + Sync> = mock as Arc<dyn Provider + Send + Sync>;
    let calls = Arc::new(AtomicU32::new(0));
    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(ToolRegistry::new())
            .config(AgentConfig {
                model: "mock-model".into(),
                max_tokens: 1024,
                auto_compact_threshold: Some(0.5),
                // Disable microcompact so it can't free tokens below threshold.
                micro_compact_enabled: false,
                ..AgentConfig::default()
            })
            .compactor(Arc::new(RecordingCompactor {
                calls: calls.clone(),
                fail: false,
            }))
            .build()
            .expect("agent should build"),
    );

    let mut stream = agent.stream_until_done(history, CancellationToken::new());
    while stream.next().await.is_some() {}
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "autocompact should have fired"
    );
}

#[tokio::test]
async fn autocompact_disables_after_two_failures() {
    use caliban_agent_core::{StopCondition, TurnEvent};
    // Force 3 turns with a failing compactor; verify it stops being called.
    let mock = Arc::new(MockProvider::new());
    mock.set_capabilities(fake_caps(100));
    // Turn 1: tool_use (loop continues); Turn 2: tool_use (loop continues); Turn 3: end.
    mock.enqueue_stream(tool_use_stream("call_1", "AgentTool", "{}"));
    mock.enqueue_stream(tool_use_stream("call_2", "AgentTool", "{}"));
    mock.enqueue_stream(text_endturn_stream("done"));
    let provider: Arc<dyn Provider + Send + Sync> = mock as Arc<dyn Provider + Send + Sync>;
    let calls = Arc::new(AtomicU32::new(0));
    let history = vec![Message::user_text("x".repeat(10_000))];
    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(ToolRegistry::new())
            .config(AgentConfig {
                model: "mock-model".into(),
                max_tokens: 1024,
                auto_compact_threshold: Some(0.1),
                micro_compact_enabled: false,
                ..AgentConfig::default()
            })
            .compactor(Arc::new(RecordingCompactor {
                calls: calls.clone(),
                fail: true,
            }))
            .build()
            .expect("agent should build"),
    );
    let mut stream = agent.stream_until_done(history, CancellationToken::new());
    let mut stopped_for: Option<StopCondition> = None;
    while let Some(ev) = stream.next().await {
        if let Ok(TurnEvent::RunEnd { stopped_for: s, .. }) = ev {
            stopped_for = Some(s);
        }
    }
    let n = calls.load(Ordering::SeqCst);
    assert!(
        (1..=2).contains(&n),
        "compactor should have been disabled after <=2 failures, got {n}",
    );
    // Crucial: a failed compaction must NOT abort the run any more.
    assert!(
        !matches!(stopped_for, Some(StopCondition::CompactionFailed(_))),
        "run should not abort on compaction failure (got {stopped_for:?})",
    );
}
