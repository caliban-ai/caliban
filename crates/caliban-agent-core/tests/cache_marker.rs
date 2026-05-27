//! Tests for the conversation-level prompt cache marker on the last user
//! message (context-management spec).

#![allow(missing_docs)]

use caliban_agent_core::cache::apply_prompt_cache;
use caliban_provider::{CacheControl, ContentBlock, Message, Tool};

#[test]
fn marks_last_user_message_when_above_threshold() {
    let mut msgs = vec![
        Message::user_text("first"),
        Message::assistant_text("reply"),
        Message::user_text("x".repeat(8000)), // ~2000 tokens (chars/4 heuristic)
    ];
    let mut tools: Vec<Tool> = Vec::new();
    apply_prompt_cache(&mut msgs, &mut tools, /*min_cache_block_tokens=*/ 1024);
    let last = &msgs[2].content[0];
    match last {
        ContentBlock::Text(t) => assert!(matches!(t.cache_control, Some(CacheControl::Ephemeral))),
        _ => panic!(),
    }
}

#[test]
fn does_not_mark_tiny_user_message() {
    let mut msgs = vec![Message::user_text("short")];
    let mut tools: Vec<Tool> = Vec::new();
    apply_prompt_cache(&mut msgs, &mut tools, 1024);
    let only = &msgs[0].content[0];
    match only {
        ContentBlock::Text(t) => assert!(t.cache_control.is_none()),
        _ => panic!(),
    }
}

#[test]
fn marks_only_last_user_not_interior() {
    let mut msgs = vec![
        Message::user_text("x".repeat(8000)),
        Message::assistant_text("reply"),
        Message::user_text("y".repeat(8000)),
    ];
    let mut tools: Vec<Tool> = Vec::new();
    apply_prompt_cache(&mut msgs, &mut tools, 1024);
    let first_user = match &msgs[0].content[0] {
        ContentBlock::Text(t) => t.cache_control,
        _ => panic!(),
    };
    let last_user = match &msgs[2].content[0] {
        ContentBlock::Text(t) => t.cache_control,
        _ => panic!(),
    };
    assert!(first_user.is_none(), "interior user must not be marked");
    assert!(matches!(last_user, Some(CacheControl::Ephemeral)));
}
