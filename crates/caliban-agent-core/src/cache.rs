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

/// Set `cache_control: Ephemeral` on the last system-message `TextBlock`
/// and on the last tool def. Mutates in place.
pub(crate) fn apply_prompt_cache(messages: &mut [Message], tools: &mut [Tool]) {
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
        apply_prompt_cache(&mut msgs, &mut tools);
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
        apply_prompt_cache(&mut msgs, &mut tools);
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
        apply_prompt_cache(&mut msgs, &mut tools);
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
        apply_prompt_cache(&mut msgs, &mut tools);
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
        apply_prompt_cache(&mut msgs, &mut tools);
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
