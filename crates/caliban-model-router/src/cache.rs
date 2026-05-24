//! Prompt-cache marker handling. When fallback crosses a route boundary,
//! `cache_control` markers are provider-specific and must be stripped from
//! the outgoing messages before the next adapter sees them.

use caliban_provider::{CompletionRequest, ContentBlock};

/// Strip every `cache_control` marker from `request`'s messages and tools.
/// Returns the number of markers cleared.
pub fn strip_cache_markers(request: &mut CompletionRequest) -> u32 {
    let mut cleared: u32 = 0;
    for msg in &mut request.messages {
        for block in &mut msg.content {
            cleared = cleared.saturating_add(strip_block(block));
        }
    }
    for tool in &mut request.tools {
        if tool.cache_control.is_some() {
            tool.cache_control = None;
            cleared = cleared.saturating_add(1);
        }
    }
    cleared
}

fn strip_block(block: &mut ContentBlock) -> u32 {
    match block {
        ContentBlock::Text(t) => {
            if t.cache_control.is_some() {
                t.cache_control = None;
                1
            } else {
                0
            }
        }
        ContentBlock::Image(i) => {
            if i.cache_control.is_some() {
                i.cache_control = None;
                1
            } else {
                0
            }
        }
        // Tool blocks don't carry cache markers in our IR.
        ContentBlock::ToolUse(_) | ContentBlock::ToolResult(_) | ContentBlock::Thinking(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{
        CacheControl, CompletionRequest, Message, Role, TextBlock, Tool, message::ContentBlock,
    };
    use serde_json::json;

    fn make_request_with_markers() -> CompletionRequest {
        CompletionRequest {
            model: "x".into(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text(TextBlock {
                        text: "hi".into(),
                        cache_control: Some(CacheControl::Ephemeral),
                    })],
                },
                Message::assistant_text("ack"),
            ],
            tools: vec![Tool {
                name: "T".into(),
                description: "d".into(),
                input_schema: json!({"type":"object"}),
                cache_control: Some(CacheControl::Ephemeral),
            }],
            tool_choice: caliban_provider::ToolChoice::default(),
            max_tokens: 64,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            metadata: Default::default(),
        }
    }

    #[test]
    fn strips_markers_on_messages_and_tools() {
        let mut req = make_request_with_markers();
        let n = strip_cache_markers(&mut req);
        assert_eq!(n, 2);
        if let ContentBlock::Text(t) = &req.messages[0].content[0] {
            assert!(t.cache_control.is_none());
        } else {
            panic!("expected text block");
        }
        assert!(req.tools[0].cache_control.is_none());
    }

    #[test]
    fn returns_zero_when_no_markers_present() {
        let mut req = CompletionRequest {
            model: "x".into(),
            messages: vec![Message::user_text("hi")],
            tools: vec![],
            tool_choice: caliban_provider::ToolChoice::default(),
            max_tokens: 64,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            metadata: Default::default(),
        };
        assert_eq!(strip_cache_markers(&mut req), 0);
    }
}
