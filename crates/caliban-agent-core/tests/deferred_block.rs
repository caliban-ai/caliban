//! Tests for `caliban_agent_core::deferred_block::splice_into_messages`
//! (ADR-0046).

use caliban_agent_core::deferred_block::splice_into_messages;
use caliban_provider::{ContentBlock, Message, Role, TextBlock};

fn sys(text: &str) -> Message {
    Message {
        role: Role::System,
        content: vec![ContentBlock::Text(TextBlock {
            text: text.to_string(),
            cache_control: None,
        })],
    }
}

fn user(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text(TextBlock {
            text: text.to_string(),
            cache_control: None,
        })],
    }
}

fn first_text(msg: &Message) -> &str {
    match msg.content.first().expect("must have content") {
        ContentBlock::Text(t) => t.text.as_str(),
        _ => panic!("expected leading text block"),
    }
}

#[test]
fn appends_to_existing_system_message_when_dropped_gt_zero() {
    let mut msgs = vec![sys("you are an agent."), user("hi")];
    splice_into_messages(&mut msgs, true, 5);
    let body = first_text(&msgs[0]);
    assert!(
        body.contains("you are an agent."),
        "preserves existing system text"
    );
    assert!(body.contains("Some MCP tools are deferred"));
    assert!(body.contains('5'), "embeds the dropped count");
}

#[test]
fn noop_when_lazy_mcp_false() {
    let mut msgs = vec![sys("foo"), user("bar")];
    splice_into_messages(&mut msgs, false, 100);
    assert_eq!(first_text(&msgs[0]), "foo");
}

#[test]
fn noop_when_dropped_zero() {
    let mut msgs = vec![sys("foo"), user("bar")];
    splice_into_messages(&mut msgs, true, 0);
    assert_eq!(first_text(&msgs[0]), "foo");
}

#[test]
fn inserts_system_message_if_none_present() {
    let mut msgs = vec![user("hi")];
    splice_into_messages(&mut msgs, true, 3);
    assert_eq!(msgs[0].role, Role::System);
    let body = first_text(&msgs[0]);
    assert!(body.contains('3'));
}

#[test]
fn empty_history_inserts_system_message() {
    let mut msgs = vec![];
    splice_into_messages(&mut msgs, true, 1);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, Role::System);
}
