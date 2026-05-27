//! Apply Anthropic-style prompt-cache markers to the system message and tools.
//!
//! `cache_control: Ephemeral` is set on:
//! - the last `TextBlock` of the system message (if any), AND
//! - the last `Tool` in the tools array (if any).
//!
//! Anthropic uses the marker to decide what to cache: everything up to and
//! including a marked block. Marking the LAST system text block + the LAST
//! tool def caches the entire stable prefix of the request (system + tools).
//!
//! For non-Anthropic providers the IR field is `Option<>` with
//! `skip_serializing_if = "Option::is_none"`, so this is a no-op on the wire.

use caliban_provider::{CacheControl, ContentBlock, Message, Role, Tool};

/// Set `cache_control: Ephemeral` on the stable prefix (last system
/// `TextBlock` + last tool def) and on the last block of the last user
/// message when that message's estimated token count is at least
/// `min_cache_block_tokens`. Mutates in place.
///
/// The user-message marker is the conversation-level cache breakpoint:
/// it makes turn N+1 reuse turn N's prefix on Anthropic, turning the
/// `cache_read` curve from flat to linear-with-history.
pub fn apply_prompt_cache(
    messages: &mut [Message],
    tools: &mut [Tool],
    min_cache_block_tokens: usize,
) {
    if let Some(sys) = messages.iter_mut().find(|m| m.role == Role::System)
        && let Some(last_text) = sys.content.iter_mut().rev().find_map(|b| match b {
            ContentBlock::Text(t) => Some(t),
            _ => None,
        })
    {
        last_text.cache_control = Some(CacheControl::Ephemeral);
    }
    if let Some(last_tool) = tools.last_mut() {
        last_tool.cache_control = Some(CacheControl::Ephemeral);
    }
    // NEW: mark last block of last user message, if it's big enough.
    if let Some(idx) = messages.iter().rposition(|m| m.role == Role::User) {
        let tokens = crate::compact::estimate_tokens(&messages[idx..=idx]);
        if (tokens as usize) >= min_cache_block_tokens
            && let Some(last_block) = messages[idx].content.last_mut()
        {
            set_cache_control_on_block(last_block, CacheControl::Ephemeral);
        }
    }
}

/// Set `cache_control` on a block when the variant supports it. Today only
/// `TextBlock` carries the field in the user-message position; other
/// variants (`Image`, `Thinking`, `ToolUse`, `ToolResult`) are no-ops
/// because the wire IR doesn't expose `cache_control` on them.
fn set_cache_control_on_block(block: &mut ContentBlock, cc: CacheControl) {
    // Other variants don't carry cache_control in the current IR; leaving
    // them unmarked is the right wire-noop behavior.
    if let ContentBlock::Text(t) = block {
        t.cache_control = Some(cc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::TextBlock;
    use serde_json::json;

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type": "object"}),
            cache_control: None,
        }
    }

    #[test]
    fn empty_inputs_do_not_panic() {
        let mut msgs: Vec<Message> = Vec::new();
        let mut tools: Vec<Tool> = Vec::new();
        apply_prompt_cache(&mut msgs, &mut tools, usize::MAX);
        assert!(msgs.is_empty());
        assert!(tools.is_empty());
    }

    #[test]
    fn marks_last_system_text_block() {
        let mut msgs = vec![Message {
            role: Role::System,
            content: vec![
                ContentBlock::Text(TextBlock {
                    text: "first".into(),
                    cache_control: None,
                }),
                ContentBlock::Text(TextBlock {
                    text: "second".into(),
                    cache_control: None,
                }),
            ],
        }];
        let mut tools: Vec<Tool> = Vec::new();
        apply_prompt_cache(&mut msgs, &mut tools, usize::MAX);
        match (&msgs[0].content[0], &msgs[0].content[1]) {
            (ContentBlock::Text(a), ContentBlock::Text(b)) => {
                assert!(a.cache_control.is_none(), "first text should be unmarked");
                assert!(
                    matches!(b.cache_control, Some(CacheControl::Ephemeral)),
                    "last text should be marked"
                );
            }
            _ => panic!("expected two text blocks"),
        }
    }

    #[test]
    fn marks_last_tool() {
        let mut msgs: Vec<Message> = Vec::new();
        let mut tools = vec![tool("a"), tool("b"), tool("c")];
        apply_prompt_cache(&mut msgs, &mut tools, usize::MAX);
        assert!(tools[0].cache_control.is_none());
        assert!(tools[1].cache_control.is_none());
        assert!(matches!(
            tools[2].cache_control,
            Some(CacheControl::Ephemeral)
        ));
    }

    #[test]
    fn system_message_without_text_blocks_is_safe() {
        let mut msgs = vec![Message {
            role: Role::System,
            content: Vec::new(),
        }];
        let mut tools = vec![tool("a")];
        apply_prompt_cache(&mut msgs, &mut tools, usize::MAX);
        assert!(matches!(
            tools[0].cache_control,
            Some(CacheControl::Ephemeral)
        ));
    }

    #[test]
    fn user_messages_unchanged() {
        let mut msgs = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text(TextBlock {
                    text: "sys".into(),
                    cache_control: None,
                })],
            },
            Message::user_text("hello"),
        ];
        let mut tools: Vec<Tool> = Vec::new();
        apply_prompt_cache(&mut msgs, &mut tools, usize::MAX);
        let user = &msgs[1];
        match &user.content[0] {
            ContentBlock::Text(t) => assert!(
                t.cache_control.is_none(),
                "user message should not be marked"
            ),
            _ => panic!("expected text"),
        }
    }
}
