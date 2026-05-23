//! Integration tests for `caliban-agent-core` using `MockProvider`.
//!
//! All 11 scenarios from the "Testing strategy" section of the spec are covered here.
//! Run with:
//!
//! ```text
//! cargo test -p caliban-agent-core --features caliban-provider/mock
//! ```

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, DropOldestCompactor, Error, HookDecision, Hooks, Message, Role,
    StopCondition, TextBlock, Tool, ToolContext, ToolError, ToolRegistry, TurnOutcome,
};
use caliban_provider::{
    Capabilities, MockProvider, PromptCachingCapability, Provider, StopReason, StreamEvent,
    StreamingContentType, StreamingDelta, SystemPromptCapability, ToolUseCapability, Usage,
};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helper: build minimal Capabilities
// ---------------------------------------------------------------------------

fn fake_caps(max_input: u32) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: 4096,
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

// ---------------------------------------------------------------------------
// Helper: stream event builders
// ---------------------------------------------------------------------------

/// Produce the complete list of `StreamEvent`s for a text-only response.
fn text_stream(
    msg_id: &str,
    model: &str,
    text: &str,
    stop: StopReason,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: model.to_owned(),
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

/// Produce `StreamEvent`s for a single `tool_use` block followed by `ToolUse` stop reason.
fn tool_use_stream(
    msg_id: &str,
    model: &str,
    tool_use_id: &str,
    tool_name: &str,
    input_json: &str,
) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: msg_id.to_owned(),
            model: model.to_owned(),
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
            delta: StreamingDelta::ToolUseInputJson(input_json.to_owned()),
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
// Helper: cast a MockProvider Arc to the trait object the builder expects
// ---------------------------------------------------------------------------

fn provider_arc(mock: Arc<MockProvider>) -> Arc<dyn Provider + Send + Sync> {
    mock as Arc<dyn Provider + Send + Sync>
}

// ---------------------------------------------------------------------------
// Helper: build a minimal agent with the given provider and optional registry
// ---------------------------------------------------------------------------

fn build_agent(mock: Arc<MockProvider>, registry: ToolRegistry) -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .provider(provider_arc(mock))
            .tools(registry)
            .model("mock-model")
            .max_tokens(1024)
            .build()
            .expect("agent should build"),
    )
}

// ---------------------------------------------------------------------------
// Simple mock tools
// ---------------------------------------------------------------------------

/// A tool that records how many times it was invoked, then returns a text block.
struct CountingTool {
    count: Arc<AtomicU32>,
    name: &'static str,
    return_text: &'static str,
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "A mock tool that counts invocations"
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
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![ContentBlock::Text(TextBlock {
            text: self.return_text.to_owned(),
            cache_control: None,
        })])
    }
}

/// A tool that always returns an execution error.
struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &'static str {
        "failing_tool"
    }

    fn description(&self) -> &'static str {
        "Always errors"
    }

    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object"}))
    }

    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        Err(ToolError::execution(std::io::Error::other("tool blew up")))
    }
}

/// A tool that waits for 5 seconds (useful for cancellation tests).
struct SlowTool;

#[async_trait]
impl Tool for SlowTool {
    fn name(&self) -> &'static str {
        "slow_tool"
    }

    fn description(&self) -> &'static str {
        "Simulates a long-running tool"
    }

    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object"}))
    }

    async fn invoke(
        &self,
        _input: serde_json::Value,
        cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        // Wait for cancellation or 5-second timeout.
        tokio::select! {
            () = cx.cancel.cancelled() => Err(ToolError::Cancelled),
            () = tokio::time::sleep(Duration::from_secs(5)) => {
                Ok(vec![ContentBlock::Text(TextBlock { text: "done".to_owned(), cache_control: None })])
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scenario 1 — single_turn_no_tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_turn_no_tools() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream(
        "msg1",
        "mock-model",
        "Hello!",
        StopReason::EndTurn,
    ));

    let agent = build_agent(Arc::clone(&mock), ToolRegistry::new());
    let outcome = agent
        .run_until_done(vec![Message::user_text("Hi")], CancellationToken::new())
        .await
        .expect("run should succeed");

    assert_eq!(outcome.turn_count, 1, "should have run exactly 1 turn");
    assert!(
        matches!(outcome.stopped_for, StopCondition::EndOfTurn),
        "expected EndOfTurn, got {:?}",
        outcome.stopped_for
    );
    // The final_messages slice: original user message + assistant reply.
    assert_eq!(outcome.final_messages.len(), 2);
    assert_eq!(outcome.final_messages[1].role, Role::Assistant);
}

// ---------------------------------------------------------------------------
// Scenario 2 — single_turn_with_tool_call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_turn_with_tool_call() {
    let mock = Arc::new(MockProvider::new());
    // Turn 1: model calls echo tool.
    mock.enqueue_stream(tool_use_stream(
        "msg1",
        "mock-model",
        "call_1",
        "echo",
        r#"{"text": "hi"}"#,
    ));
    // Turn 2: model responds with text.
    mock.enqueue_stream(text_stream(
        "msg2",
        "mock-model",
        "Done!",
        StopReason::EndTurn,
    ));

    let invocations = Arc::new(AtomicU32::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CountingTool {
        count: Arc::clone(&invocations),
        name: "echo",
        return_text: "echoed: hi",
    }));

    let agent = build_agent(Arc::clone(&mock), registry);
    let outcome = agent
        .run_until_done(
            vec![Message::user_text("Use the echo tool.")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    assert_eq!(
        invocations.load(Ordering::SeqCst),
        1,
        "tool should be called once"
    );
    assert_eq!(outcome.turn_count, 2);
    assert!(matches!(outcome.stopped_for, StopCondition::EndOfTurn));

    // Expected history: user + assistant(tool_use) + user(tool_result) + assistant(text)
    assert_eq!(outcome.final_messages.len(), 4);
    assert_eq!(outcome.final_messages[0].role, Role::User);
    assert_eq!(outcome.final_messages[1].role, Role::Assistant);
    assert_eq!(outcome.final_messages[2].role, Role::User);
    assert_eq!(outcome.final_messages[3].role, Role::Assistant);

    // Verify usage is summed across turns.
    assert!(outcome.total_usage.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// Scenario 3 — tool_call_with_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_call_with_error() {
    let mock = Arc::new(MockProvider::new());
    // Turn 1: model calls the failing tool.
    mock.enqueue_stream(tool_use_stream(
        "msg1",
        "mock-model",
        "call_err",
        "failing_tool",
        "{}",
    ));
    // Turn 2: model sees the error and responds normally.
    mock.enqueue_stream(text_stream(
        "msg2",
        "mock-model",
        "Sorry about that.",
        StopReason::EndTurn,
    ));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FailingTool));

    let agent = build_agent(Arc::clone(&mock), registry);
    let outcome = agent
        .run_until_done(
            vec![Message::user_text("call failing_tool")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed despite tool error");

    assert!(matches!(outcome.stopped_for, StopCondition::EndOfTurn));
    // 4 messages: user + assistant(tool_use) + user(tool_result with error) + assistant(text)
    assert_eq!(outcome.final_messages.len(), 4);

    // The tool-result message should be a User message with is_error: true.
    let tool_result_msg = &outcome.final_messages[2];
    assert_eq!(tool_result_msg.role, Role::User);
    let has_error_block = tool_result_msg.content.iter().any(|b| {
        if let ContentBlock::ToolResult(tr) = b {
            tr.is_error
        } else {
            false
        }
    });
    assert!(has_error_block, "tool result should be marked is_error");

    // Verify error text is present.
    let error_text_present = tool_result_msg.content.iter().any(|b| {
        if let ContentBlock::ToolResult(tr) = b {
            tr.content.iter().any(|cb| {
                if let ContentBlock::Text(t) = cb {
                    t.text.contains("Error:") || t.text.contains("tool blew up")
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(error_text_present, "error text should be in tool result");
}

// ---------------------------------------------------------------------------
// Scenario 4 — multi_turn_tool_chain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_turn_tool_chain() {
    let mock = Arc::new(MockProvider::new());
    // Turn 1: model calls echo.
    mock.enqueue_stream(tool_use_stream(
        "msg1",
        "mock-model",
        "call_a",
        "echo",
        r#"{"text":"a"}"#,
    ));
    // Turn 2: model calls echo again.
    mock.enqueue_stream(tool_use_stream(
        "msg2",
        "mock-model",
        "call_b",
        "echo",
        r#"{"text":"b"}"#,
    ));
    // Turn 3: model ends.
    mock.enqueue_stream(text_stream(
        "msg3",
        "mock-model",
        "All done.",
        StopReason::EndTurn,
    ));

    let invocations = Arc::new(AtomicU32::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CountingTool {
        count: Arc::clone(&invocations),
        name: "echo",
        return_text: "ok",
    }));

    let agent = build_agent(Arc::clone(&mock), registry);
    let outcome = agent
        .run_until_done(
            vec![Message::user_text("call the tool twice")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    assert_eq!(invocations.load(Ordering::SeqCst), 2, "tool called twice");
    assert_eq!(outcome.turn_count, 3, "should have run 3 turns");
    assert!(matches!(outcome.stopped_for, StopCondition::EndOfTurn));

    // user + (asst + user_result) × 2 + asst = 6 messages total
    assert_eq!(outcome.final_messages.len(), 6);
}

// ---------------------------------------------------------------------------
// Scenario 5 — cancellation_mid_turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancellation_mid_turn() {
    let mock = Arc::new(MockProvider::new());
    // The model calls the slow tool.
    mock.enqueue_stream(tool_use_stream(
        "msg1",
        "mock-model",
        "call_slow",
        "slow_tool",
        "{}",
    ));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SlowTool));

    let agent = build_agent(Arc::clone(&mock), registry);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Cancel after 100 ms.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_clone.cancel();
    });

    let start = tokio::time::Instant::now();
    let result = agent
        .run_until_done(vec![Message::user_text("run the slow tool")], cancel)
        .await;

    let elapsed = start.elapsed();

    // Should come back with Cancelled (as a RunOutcome, not an Err, since the stream
    // emits RunEnd with StopCondition::Cancelled).
    match result {
        Ok(outcome) => {
            assert!(
                matches!(outcome.stopped_for, StopCondition::Cancelled),
                "expected Cancelled, got {:?}",
                outcome.stopped_for
            );
        }
        Err(e) => {
            // Cancellation may surface as Error::Cancelled too — both are valid.
            assert!(matches!(e, Error::Cancelled), "unexpected error: {e}");
        }
    }

    assert!(
        elapsed < Duration::from_millis(500),
        "cancelled run took too long: {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 6 — max_turns_reached
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_turns_reached() {
    let mock = Arc::new(MockProvider::new());
    // Enqueue many tool_use responses — more than max_turns.
    for i in 0..5u32 {
        mock.enqueue_stream(tool_use_stream(
            &format!("msg{i}"),
            "mock-model",
            &format!("call_{i}"),
            "echo",
            "{}",
        ));
    }

    let mut registry = ToolRegistry::new();
    let invocations = Arc::new(AtomicU32::new(0));
    registry.register(Arc::new(CountingTool {
        count: Arc::clone(&invocations),
        name: "echo",
        return_text: "ok",
    }));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .tools(registry)
            .model("mock-model")
            .max_tokens(1024)
            .max_turns(3)
            .build()
            .expect("agent should build"),
    );

    let outcome = agent
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("should return outcome (not Err)");

    assert!(
        matches!(outcome.stopped_for, StopCondition::MaxTurnsReached(3)),
        "expected MaxTurnsReached(3), got {:?}",
        outcome.stopped_for
    );
    assert_eq!(outcome.turn_count, 3);
}

// ---------------------------------------------------------------------------
// Scenario 7 — retry_on_rate_limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_on_rate_limit() {
    use caliban_agent_core::RetryPolicy;

    let mock = Arc::new(MockProvider::new());
    // Two rate-limit errors from the stream() call itself, then success.
    mock.enqueue_stream_error(caliban_provider::Error::RateLimit {
        retry_after: Some(Duration::from_millis(10)),
    });
    mock.enqueue_stream_error(caliban_provider::Error::RateLimit {
        retry_after: Some(Duration::from_millis(10)),
    });
    mock.enqueue_stream(text_stream(
        "msg1",
        "mock-model",
        "OK!",
        StopReason::EndTurn,
    ));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .model("mock-model")
            .max_tokens(1024)
            .retry_policy(RetryPolicy {
                max_attempts: 3,
                initial_backoff: Duration::from_millis(5),
                backoff_multiplier: 1.0,
                max_backoff: Duration::from_millis(20),
                jitter: false,
            })
            .build()
            .expect("agent should build"),
    );

    let start = tokio::time::Instant::now();
    let outcome = agent
        .run_until_done(vec![Message::user_text("hello")], CancellationToken::new())
        .await
        .expect("run should succeed after retries");

    let elapsed = start.elapsed();
    assert!(
        matches!(outcome.stopped_for, StopCondition::EndOfTurn),
        "expected EndOfTurn, got {:?}",
        outcome.stopped_for
    );
    // We slept at least 10ms + 10ms = 20ms (retry_after durations).
    assert!(
        elapsed >= Duration::from_millis(15),
        "retries should have waited; elapsed: {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 8 — retry_not_attempted_on_auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_not_attempted_on_auth() {
    use caliban_agent_core::RetryPolicy;

    let mock = Arc::new(MockProvider::new());
    // Auth error — not retryable. Enqueue a success after it to confirm the
    // agent does NOT consume it (i.e., it gave up after the first attempt).
    mock.enqueue_stream_error(caliban_provider::Error::Auth("bad key".into()));
    mock.enqueue_stream(text_stream(
        "msg1",
        "mock-model",
        "never reached",
        StopReason::EndTurn,
    ));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .model("mock-model")
            .max_tokens(1024)
            .retry_policy(RetryPolicy {
                max_attempts: 3,
                ..RetryPolicy::default()
            })
            .build()
            .expect("agent should build"),
    );

    let outcome = agent
        .run_until_done(vec![Message::user_text("hello")], CancellationToken::new())
        .await
        .expect("should return RunOutcome, not Err");

    // The run should have stopped with a ProviderError — auth was not retried.
    assert!(
        matches!(outcome.stopped_for, StopCondition::ProviderError(_)),
        "expected ProviderError, got {:?}",
        outcome.stopped_for
    );
    if let StopCondition::ProviderError(msg) = &outcome.stopped_for {
        assert!(
            msg.contains("authentication") || msg.contains("bad key"),
            "error should mention auth: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 9 — hook_denies_tool
// ---------------------------------------------------------------------------

struct DenyAllHooks;

#[async_trait]
impl Hooks for DenyAllHooks {
    async fn before_tool(
        &self,
        _ctx: &caliban_agent_core::ToolCtx<'_>,
    ) -> caliban_agent_core::Result<HookDecision> {
        Ok(HookDecision::Deny("not authorized".to_owned()))
    }
}

#[tokio::test]
async fn hook_denies_tool() {
    let mock = Arc::new(MockProvider::new());
    // Turn 1: model calls echo.
    mock.enqueue_stream(tool_use_stream(
        "msg1",
        "mock-model",
        "call_denied",
        "echo",
        "{}",
    ));
    // Turn 2: model sees the denial and ends.
    mock.enqueue_stream(text_stream(
        "msg2",
        "mock-model",
        "Understood.",
        StopReason::EndTurn,
    ));

    let invocations = Arc::new(AtomicU32::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CountingTool {
        count: Arc::clone(&invocations),
        name: "echo",
        return_text: "should not be returned",
    }));

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .tools(registry)
            .model("mock-model")
            .max_tokens(1024)
            .hooks(Arc::new(DenyAllHooks))
            .build()
            .expect("agent should build"),
    );

    let outcome = agent
        .run_until_done(
            vec![Message::user_text("use echo")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    // Tool was NOT invoked.
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        0,
        "tool must not be invoked"
    );

    // Run completed normally.
    assert!(matches!(outcome.stopped_for, StopCondition::EndOfTurn));

    // The tool-result message should contain the denial text and is_error: true.
    let tool_result_msg = outcome
        .final_messages
        .iter()
        .find(|m| {
            m.role == Role::User
                && m.content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult(_)))
        })
        .expect("should have a tool-result message");

    let denial_correct = tool_result_msg.content.iter().any(|b| {
        if let ContentBlock::ToolResult(tr) = b {
            let text_ok = tr.content.iter().any(|cb| {
                if let ContentBlock::Text(t) = cb {
                    t.text.contains("not authorized") || t.text.contains("denied")
                } else {
                    false
                }
            });
            tr.is_error && text_ok
        } else {
            false
        }
    });
    assert!(
        denial_correct,
        "tool result should be is_error with denial message"
    );
}

// ---------------------------------------------------------------------------
// Scenario 10 — compaction_triggered
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compaction_triggered() {
    let mock = Arc::new(MockProvider::new());
    // Mock caps: small context window so DropOldestCompactor triggers.
    mock.set_capabilities(fake_caps(200));

    // Single turn: model responds with text.
    mock.enqueue_stream(text_stream(
        "msg1",
        "mock-model",
        "Compacted!",
        StopReason::EndTurn,
    ));

    let compactor = Arc::new(DropOldestCompactor {
        target_fraction: 0.5,
        keep_recent_turns: 1,
    });

    let agent = Arc::new(
        Agent::builder()
            .provider(provider_arc(Arc::clone(&mock)))
            .model("mock-model")
            .max_tokens(1024)
            .compactor(compactor)
            .build()
            .expect("agent should build"),
    );

    // Build an initial history that exceeds the 200 * 0.5 = 100 token threshold.
    // Each message with 100 chars ≈ 25 tokens; 5 pairs → ~250 tokens → exceeds threshold.
    let mut initial_messages = vec![Message::system_text("rules")];
    for i in 0..5u32 {
        initial_messages.push(Message::user_text(format!(
            "question {i}: {}",
            "x".repeat(100)
        )));
        initial_messages.push(Message::assistant_text(format!(
            "answer {i}: {}",
            "x".repeat(100)
        )));
    }
    let initial_len = initial_messages.len();

    let outcome = agent
        .run_until_done(initial_messages, CancellationToken::new())
        .await
        .expect("run should succeed");

    assert!(matches!(outcome.stopped_for, StopCondition::EndOfTurn));
    // After compaction + 1 assistant response, the final history should be shorter
    // than the initial history + 1 assistant message would have been.
    // The compactor drops old messages, so final_messages < initial_len + 1.
    assert!(
        outcome.final_messages.len() < initial_len + 1,
        "expected compaction to shorten history; initial={initial_len}, final={}",
        outcome.final_messages.len()
    );
}

// ---------------------------------------------------------------------------
// Scenario 11 — run_turn returns TurnOutcome correctly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_turn_returns_turn_outcome() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream(
        "msg1",
        "mock-model",
        "One turn!",
        StopReason::EndTurn,
    ));

    let agent = build_agent(Arc::clone(&mock), ToolRegistry::new());
    let outcome: TurnOutcome = agent
        .run_turn(
            vec![Message::user_text("one turn please")],
            CancellationToken::new(),
        )
        .await
        .expect("run_turn should succeed");

    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert!(!outcome.continue_loop);
    assert_eq!(outcome.tool_results.len(), 0);
    assert_eq!(outcome.assistant_message.role, Role::Assistant);
}

// ---------------------------------------------------------------------------
// Scenario 12 — property: final_messages starts with input prefix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn final_messages_starts_with_input_prefix() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream(
        "msg1",
        "mock-model",
        "Reply",
        StopReason::EndTurn,
    ));

    let initial = vec![
        Message::system_text("You are helpful."),
        Message::user_text("Hello"),
    ];
    let agent = build_agent(Arc::clone(&mock), ToolRegistry::new());
    let outcome = agent
        .run_until_done(initial.clone(), CancellationToken::new())
        .await
        .expect("run should succeed");

    for (i, expected) in initial.iter().enumerate() {
        assert_eq!(
            &outcome.final_messages[i], expected,
            "message {i} should match the input prefix"
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 13 — property: tool result messages always follow assistant messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_result_follows_assistant_message() {
    let mock = Arc::new(MockProvider::new());
    // Turn with tool use.
    mock.enqueue_stream(tool_use_stream(
        "msg1",
        "mock-model",
        "call_prop",
        "echo",
        "{}",
    ));
    // End turn.
    mock.enqueue_stream(text_stream(
        "msg2",
        "mock-model",
        "Done",
        StopReason::EndTurn,
    ));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CountingTool {
        count: Arc::new(AtomicU32::new(0)),
        name: "echo",
        return_text: "result",
    }));

    let agent = build_agent(Arc::clone(&mock), registry);
    let outcome = agent
        .run_until_done(vec![Message::user_text("go")], CancellationToken::new())
        .await
        .expect("run should succeed");

    // Find every ToolResult block — the preceding message must be Assistant.
    let msgs = &outcome.final_messages;
    for (i, msg) in msgs.iter().enumerate() {
        let has_tool_result = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult(_)));
        if has_tool_result {
            assert!(i > 0, "tool result message cannot be the first message");
            assert_eq!(
                msgs[i - 1].role,
                Role::Assistant,
                "message before tool result (index {i}) must be Assistant"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Scenario 14 — tool_use_ids match one-to-one
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_use_ids_match_tool_results() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(tool_use_stream(
        "msg1",
        "mock-model",
        "unique_id_42",
        "echo",
        "{}",
    ));
    mock.enqueue_stream(text_stream(
        "msg2",
        "mock-model",
        "Done",
        StopReason::EndTurn,
    ));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CountingTool {
        count: Arc::new(AtomicU32::new(0)),
        name: "echo",
        return_text: "res",
    }));

    let agent = build_agent(Arc::clone(&mock), registry);
    let outcome = agent
        .run_until_done(
            vec![Message::user_text("call echo")],
            CancellationToken::new(),
        )
        .await
        .expect("run should succeed");

    // Collect all tool_use_ids from assistant messages.
    let mut use_ids: Vec<String> = Vec::new();
    for msg in &outcome.final_messages {
        if msg.role == Role::Assistant {
            for block in &msg.content {
                if let ContentBlock::ToolUse(tu) = block {
                    use_ids.push(tu.id.clone());
                }
            }
        }
    }

    // Collect all tool_result ids from user messages.
    let mut result_ids: Vec<String> = Vec::new();
    for msg in &outcome.final_messages {
        if msg.role == Role::User {
            for block in &msg.content {
                if let ContentBlock::ToolResult(tr) = block {
                    result_ids.push(tr.tool_use_id.clone());
                }
            }
        }
    }

    assert_eq!(
        use_ids, result_ids,
        "tool_use IDs must match tool_result IDs"
    );
    assert!(use_ids.contains(&"unique_id_42".to_owned()));
}
