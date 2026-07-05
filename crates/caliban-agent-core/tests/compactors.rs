#![allow(missing_docs)]

use caliban_agent_core::compact::{Compactor, DropOldestCompactor, NoopCompactor, estimate_tokens};
use caliban_provider::{
    Capabilities, Message, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};

fn fake_caps(max_input: u32) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: 1024,
        vision: false,
        tool_use: ToolUseCapability::Basic,
        thinking: false,
        prompt_caching: PromptCachingCapability::None,
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: SystemPromptCapability::SystemRole,
        refusal_field: false,
    }
}

#[tokio::test]
async fn noop_always_returns_none() {
    let c = NoopCompactor;
    let result = c
        .compact(&[Message::user_text("hi")], &fake_caps(100_000))
        .await
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn estimate_tokens_smoke() {
    let m = Message::user_text("x".repeat(4000));
    assert!(estimate_tokens(&[m]) >= 999);
}

#[tokio::test]
async fn drop_oldest_is_noop_below_threshold() {
    let c = DropOldestCompactor::default();
    let messages = vec![Message::user_text("hi"), Message::assistant_text("hi back")];
    let result = c.compact(&messages, &fake_caps(100_000)).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn drop_oldest_truncates_above_threshold() {
    let c = DropOldestCompactor {
        target_fraction: 0.5,
        keep_recent_turns: 1,
    };
    let mut messages = vec![Message::system_text("rules")];
    for i in 0..20 {
        messages.push(Message::user_text(format!("q{i}: {}", "x".repeat(200))));
        messages.push(Message::assistant_text(format!(
            "a{i}: {}",
            "x".repeat(200)
        )));
    }
    let result = c
        .compact(&messages, &fake_caps(2000))
        .await
        .unwrap()
        .unwrap()
        .messages;
    // Should preserve leading System + at most 2 most-recent messages (1 turn = 2 messages).
    assert!(result.len() <= 3);
    assert_eq!(result[0].role, caliban_provider::Role::System);
}
