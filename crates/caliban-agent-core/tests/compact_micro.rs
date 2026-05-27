//! Unit + end-to-end tests for `MicroCompactor` (LLM-free supersession).

#![allow(missing_docs)]

use caliban_agent_core::compact::{Compactor, MicroCompactor};
use caliban_provider::{
    Capabilities, ContentBlock, Message, PromptCachingCapability, Role, SystemPromptCapability,
    TextBlock, ToolResultBlock, ToolUseBlock, ToolUseCapability,
};
use serde_json::json;

fn capabilities() -> Capabilities {
    Capabilities {
        max_input_tokens: 100_000,
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

fn read_use(id: &str, path: &str) -> ContentBlock {
    ContentBlock::ToolUse(ToolUseBlock {
        id: id.into(),
        name: "Read".into(),
        input: json!({ "file_path": path }),
    })
}

fn tool_result(id: &str, body: &str) -> ContentBlock {
    ContentBlock::ToolResult(ToolResultBlock {
        tool_use_id: id.into(),
        content: vec![ContentBlock::Text(TextBlock {
            text: body.into(),
            cache_control: None,
        })],
        is_error: false,
    })
}

#[tokio::test]
async fn supersedes_older_read_of_same_path() {
    let msgs = vec![
        Message {
            role: Role::Assistant,
            content: vec![read_use("a", "/x.rs")],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("a", "old content")],
        },
        Message {
            role: Role::Assistant,
            content: vec![read_use("b", "/x.rs")],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("b", "new content")],
        },
    ];
    let out = MicroCompactor::new()
        .compact(&msgs, &capabilities())
        .await
        .unwrap();
    let new = out.expect("microcompact should mutate");
    // Older result replaced with placeholder
    let text_a = match &new[1].content[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert!(text_a.starts_with("[superseded: Read("));
    // Newer result preserved
    let text_b = match &new[3].content[0] {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert_eq!(text_b, "new content");
}

#[tokio::test]
async fn does_not_supersede_different_path() {
    let msgs = vec![
        Message {
            role: Role::Assistant,
            content: vec![read_use("a", "/x.rs")],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("a", "X")],
        },
        Message {
            role: Role::Assistant,
            content: vec![read_use("b", "/y.rs")],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("b", "Y")],
        },
    ];
    let out = MicroCompactor::new()
        .compact(&msgs, &capabilities())
        .await
        .unwrap();
    assert!(out.is_none(), "no supersession across different paths");
}

// ---------------------------------------------------------------------------
// End-to-end test: microcompact runs at the top of each turn.
// ---------------------------------------------------------------------------

use caliban_agent_core::{Agent, AgentConfig, ToolRegistry};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn text_stream_endturn(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
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

#[tokio::test]
async fn microcompact_runs_pre_turn_and_strips_superseded() {
    // History already contains two reads of /x.rs from previous turns.
    let history = vec![
        Message::user_text("read this please"),
        Message {
            role: Role::Assistant,
            content: vec![read_use("a", "/x.rs")],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("a", "v1")],
        },
        Message {
            role: Role::Assistant,
            content: vec![read_use("b", "/x.rs")],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("b", "v2")],
        },
    ];
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(text_stream_endturn("ok"));
    let provider: Arc<dyn Provider + Send + Sync> = mock as Arc<dyn Provider + Send + Sync>;

    let cfg = AgentConfig {
        model: "mock-model".into(),
        max_tokens: 1024,
        micro_compact_enabled: true,
        ..AgentConfig::default()
    };
    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .tools(ToolRegistry::new())
            .config(cfg)
            .build()
            .expect("agent should build"),
    );

    let mut stream = agent.stream_until_done(history, CancellationToken::new());
    let mut final_messages = Vec::new();
    while let Some(ev) = stream.next().await {
        if let Ok(caliban_agent_core::TurnEvent::RunEnd {
            final_messages: fm, ..
        }) = ev
        {
            final_messages = fm;
        }
    }
    // Index 2 is the older tool_result; it should be superseded.
    let old_result = &final_messages[2].content[0];
    let text = match old_result {
        ContentBlock::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text(t) => t.text.clone(),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert!(
        text.starts_with("[superseded:"),
        "expected superseded placeholder, got: {text}"
    );
}

#[tokio::test]
async fn does_not_supersede_bash() {
    let msgs = vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse(ToolUseBlock {
                id: "a".into(),
                name: "Bash".into(),
                input: json!({ "command": "ls" }),
            })],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("a", "out1")],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse(ToolUseBlock {
                id: "b".into(),
                name: "Bash".into(),
                input: json!({ "command": "ls" }),
            })],
        },
        Message {
            role: Role::User,
            content: vec![tool_result("b", "out2")],
        },
    ];
    let out = MicroCompactor::new()
        .compact(&msgs, &capabilities())
        .await
        .unwrap();
    assert!(out.is_none());
}
