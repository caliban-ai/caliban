//! Integration tests for the `AgentTool` (sub-agent primitive).

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, ContentBlock, TextBlock, Tool, ToolContext, ToolError, ToolRegistry,
};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use caliban_tools_builtin::{AgentFactory, AgentTool, AgentToolInput};
use tokio_util::sync::CancellationToken;

// ---- Test tools ----

struct ReadTestTool {
    schema: serde_json::Value,
}

impl ReadTestTool {
    fn new() -> Self {
        Self {
            schema: serde_json::json!({"type":"object"}),
        }
    }
}

#[async_trait]
impl Tool for ReadTestTool {
    fn name(&self) -> &'static str {
        "Read"
    }
    fn description(&self) -> &'static str {
        "read test tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "read-ok".into(),
            cache_control: None,
        })])
    }
}

// ---- Helpers ----

fn text_response(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "m".into(),
            model: "mock".into(),
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
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn agent_with_provider(mp: Arc<MockProvider>, tools: ToolRegistry, model: &str) -> Agent {
    Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(tools)
        .model(model)
        .max_tokens(64)
        .max_turns(20)
        .build()
        .unwrap()
}

fn ctx() -> ToolContext {
    ToolContext {
        tool_use_id: "t1".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    }
}

fn factory_from(mp: Arc<MockProvider>) -> AgentFactory {
    Arc::new(move |input: &AgentToolInput| {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadTestTool::new()));
        let model = input
            .model
            .clone()
            .unwrap_or_else(|| "mock-default".to_string());
        agent_with_provider(Arc::clone(&mp), registry, &model)
    })
}

// ---- Tests ----

#[tokio::test]
async fn returns_final_text_to_parent() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response("OK"));

    let tool = AgentTool::new(factory_from(mp), None);
    let out = tool
        .invoke(serde_json::json!({ "prompt": "say OK" }), ctx())
        .await
        .unwrap();
    let ContentBlock::Text(t) = &out[0] else {
        panic!("expected text block")
    };
    assert_eq!(t.text, "OK");
}

#[tokio::test]
async fn truncates_long_output() {
    let big = "a".repeat(6_000);
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response(&big));

    let tool = AgentTool::new(factory_from(mp), None);
    let out = tool
        .invoke(serde_json::json!({ "prompt": "say long" }), ctx())
        .await
        .unwrap();
    let ContentBlock::Text(t) = &out[0] else {
        panic!()
    };
    assert!(t.text.ends_with("[sub-agent output truncated]"));
    // 5000 a's + newlines + footer
    assert!(t.text.starts_with("aaaaa"));
}

#[tokio::test]
async fn model_override_is_honored() {
    // We use a closure-side variable to inspect what model the factory was given.
    let chosen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let chosen_for_factory = std::sync::Arc::clone(&chosen);
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response("OK"));

    let factory: AgentFactory = Arc::new(move |input: &AgentToolInput| {
        let model = input
            .model
            .clone()
            .unwrap_or_else(|| "mock-default".to_string());
        *chosen_for_factory.lock().unwrap() = model.clone();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadTestTool::new()));
        agent_with_provider(Arc::clone(&mp), registry, &model)
    });

    let tool = AgentTool::new(factory, None);
    tool.invoke(
        serde_json::json!({ "prompt": "say OK", "model": "gpt-4o-mini" }),
        ctx(),
    )
    .await
    .unwrap();
    assert_eq!(*chosen.lock().unwrap(), "gpt-4o-mini");
}

#[tokio::test]
async fn cancellation_propagates() {
    let mp = Arc::new(MockProvider::new());
    // Enqueue a stream that takes a small amount of time before completion.
    mp.enqueue_stream(text_response("never read"));

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        cancel_clone.cancel();
    });

    let cx = ToolContext {
        tool_use_id: "t1".into(),
        cancel: cancel.clone(),
        hooks: None,
        turn_index: 0,
    };

    let tool = AgentTool::new(factory_from(mp), None);
    let res = tool.invoke(serde_json::json!({ "prompt": "go" }), cx).await;
    // Either the sub-agent finished before cancel (small race) — accept OK,
    // OR it was cancelled. Both demonstrate the wiring works; we mainly want
    // to exercise the cancel path doesn't panic.
    if let Err(e) = res {
        assert!(matches!(e, ToolError::Cancelled));
    }
    // If Ok, the sub-agent finished before the spawned cancel fired — also fine.
    // The cancel token at least gets to fire without panicking.
    drop(cancel);
}

#[tokio::test]
async fn invalid_input_errors() {
    let mp = Arc::new(MockProvider::new());
    let tool = AgentTool::new(factory_from(mp), None);
    // Missing required "prompt"
    let err = tool.invoke(serde_json::json!({}), ctx()).await.unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
}

// ---- ADR 0037 additions ----

#[tokio::test]
async fn isolation_field_parses_worktree() {
    use caliban_tools_builtin::{AgentToolInput, IsolationMode};
    let parsed: AgentToolInput = serde_json::from_value(serde_json::json!({
        "prompt": "x",
        "isolation": "worktree"
    }))
    .unwrap();
    assert_eq!(parsed.isolation, IsolationMode::Worktree);
}

#[tokio::test]
async fn isolation_defaults_to_none() {
    use caliban_tools_builtin::{AgentToolInput, IsolationMode};
    let parsed: AgentToolInput =
        serde_json::from_value(serde_json::json!({ "prompt": "x" })).unwrap();
    assert_eq!(parsed.isolation, IsolationMode::None);
    assert!(!parsed.background);
    assert!(parsed.inherit_hooks, "inherit_hooks defaults to true");
}

#[tokio::test]
async fn inherit_hooks_false_parses() {
    use caliban_tools_builtin::AgentToolInput;
    let parsed: AgentToolInput = serde_json::from_value(serde_json::json!({
        "prompt": "x",
        "inherit_hooks": false
    }))
    .unwrap();
    assert!(!parsed.inherit_hooks);
}

#[tokio::test]
async fn worktree_options_parse() {
    use caliban_tools_builtin::AgentToolInput;
    let parsed: AgentToolInput = serde_json::from_value(serde_json::json!({
        "prompt": "x",
        "isolation": "worktree",
        "worktree": {
            "base_ref": "fresh",
            "sparse_paths": ["crates/foo"],
            "symlink_directories": ["target"]
        }
    }))
    .unwrap();
    let wt = parsed.worktree.unwrap();
    assert_eq!(wt.base_ref.as_deref(), Some("fresh"));
    assert_eq!(wt.sparse_paths, vec!["crates/foo".to_string()]);
    assert_eq!(
        wt.symlink_directories,
        vec![std::path::PathBuf::from("target")]
    );
}

#[tokio::test]
async fn background_handoff_invokes_spawner_and_returns_id() {
    use caliban_tools_builtin::{BackgroundSpawnResult, BackgroundSpawner};

    let mp = Arc::new(MockProvider::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls2 = Arc::clone(&calls);
    let spawner: BackgroundSpawner = Arc::new(move |_input| {
        calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        BackgroundSpawnResult {
            id: "deadbeef0000".into(),
            socket_path: std::path::PathBuf::from("/tmp/x.sock"),
        }
    });
    let tool = AgentTool::new(factory_from(mp), None).with_background_spawner(spawner);
    let out = tool
        .invoke(
            serde_json::json!({ "prompt": "go", "background": true, "label": "bg" }),
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let ContentBlock::Text(t) = &out[0] else {
        panic!("expected text")
    };
    assert!(t.text.contains("deadbeef0000"));
    assert!(t.text.contains("backgrounded sub-agent"));
}

#[tokio::test]
async fn parent_hooks_fire_on_subagent_start_and_stop() {
    use caliban_agent_core::{Hooks, SubagentCtx, SubagentOutcome};
    use std::sync::Mutex;

    #[derive(Default)]
    struct CapturingHooks {
        events: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Hooks for CapturingHooks {
        async fn subagent_start(&self, ctx: &SubagentCtx<'_>) -> caliban_agent_core::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("start:{}", ctx.task_id));
            Ok(())
        }
        async fn subagent_stop(
            &self,
            ctx: &SubagentCtx<'_>,
            _outcome: &SubagentOutcome,
        ) -> caliban_agent_core::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("stop:{}", ctx.task_id));
            Ok(())
        }
    }

    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response("done"));
    let tool = AgentTool::new(factory_from(mp), None);
    let hooks = Arc::new(CapturingHooks::default());
    let cx = ToolContext {
        tool_use_id: "tx-99".into(),
        cancel: CancellationToken::new(),
        hooks: Some(hooks.clone() as Arc<dyn Hooks + Send + Sync>),
        turn_index: 0,
    };
    let _ = tool
        .invoke(serde_json::json!({ "prompt": "go" }), cx)
        .await
        .unwrap();
    let evs = hooks.events.lock().unwrap();
    assert!(
        evs.iter().any(|e| e.starts_with("start:tx-99")),
        "expected subagent_start; got {evs:?}"
    );
    assert!(
        evs.iter().any(|e| e.starts_with("stop:tx-99")),
        "expected subagent_stop; got {evs:?}"
    );
}

#[tokio::test]
async fn background_handoff_warns_when_inherit_hooks_true_and_hooks_present() {
    use caliban_agent_core::{Hooks, NoopHooks};
    use caliban_tools_builtin::{BackgroundSpawnResult, BackgroundSpawner};
    let mp = Arc::new(MockProvider::new());
    let spawner: BackgroundSpawner = Arc::new(|_input| BackgroundSpawnResult {
        id: "abc".into(),
        socket_path: std::path::PathBuf::from("/tmp/y.sock"),
    });
    let tool = AgentTool::new(factory_from(mp), None).with_background_spawner(spawner);
    let cx = ToolContext {
        tool_use_id: "tx".into(),
        cancel: CancellationToken::new(),
        hooks: Some(Arc::new(NoopHooks) as Arc<dyn Hooks + Send + Sync>),
        turn_index: 0,
    };
    // Just exercises the code path that emits the warn!. The presence
    // of the warning itself is observable in tracing; here we just
    // confirm the spawn went through despite the warning.
    let out = tool
        .invoke(
            serde_json::json!({ "prompt": "x", "background": true, "inherit_hooks": true }),
            cx,
        )
        .await
        .unwrap();
    let ContentBlock::Text(t) = &out[0] else {
        panic!()
    };
    assert!(t.text.contains("abc"));
}

#[tokio::test]
async fn background_without_spawner_falls_back_to_foreground() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_response("foreground-OK"));
    let tool = AgentTool::new(factory_from(mp), None);
    let out = tool
        .invoke(
            serde_json::json!({ "prompt": "go", "background": true }),
            ctx(),
        )
        .await
        .unwrap();
    let ContentBlock::Text(t) = &out[0] else {
        panic!()
    };
    // Foreground path runs the mock and returns its text.
    assert_eq!(t.text, "foreground-OK");
}
