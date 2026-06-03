//! System-prompt splice for the lazy-MCP deferred-block paragraph
//! (ADR-0046).
//!
//! When `tools.lazy_mcp = true` and the per-turn wire filter drops at
//! least one MCP tool, we splice a fixed paragraph into the leading
//! system message so the model knows the affordance exists. This is a
//! belt-and-suspenders complement to the `ToolSearch` tool's own
//! description: the model should be able to discover the search-then-
//! call pattern from either signal alone.

use caliban_provider::{ContentBlock, Message, Role, TextBlock};

const DEFERRED_BLOCK_TEMPLATE: &str = "Some MCP tools are deferred to keep your tool palette lean. \
     Use the `ToolSearch` tool with a substring query to discover \
     and activate them when needed; activated tools persist for the \
     rest of the session. {N} MCP tools are currently deferred.";

fn format_block(dropped: usize) -> String {
    DEFERRED_BLOCK_TEMPLATE.replace("{N}", &dropped.to_string())
}

/// Splice the deferred-block paragraph into the leading system message
/// of `messages`. No-op when `lazy_mcp` is false or `dropped` is 0.
///
/// Behavior:
/// - If the first message is a `Role::System` text message, the block
///   is appended to its leading text block (separated by a blank line).
/// - If the first message is `Role::System` but its leading content
///   block is not text, a new text block is inserted at the front.
/// - If no leading system message exists, one is inserted at index 0.
///
/// Position invariant matches ADR-0008: system messages remain
/// leading-only.
pub fn splice_into_messages(messages: &mut Vec<Message>, lazy_mcp: bool, dropped: usize) {
    if !lazy_mcp || dropped == 0 {
        return;
    }
    let block = format_block(dropped);

    if let Some(first) = messages.first_mut()
        && matches!(first.role, Role::System)
    {
        if let Some(ContentBlock::Text(t)) = first.content.first_mut() {
            t.text.push_str("\n\n");
            t.text.push_str(&block);
        } else {
            first.content.insert(
                0,
                ContentBlock::Text(TextBlock {
                    text: block.clone(),
                    cache_control: None,
                }),
            );
        }
        return;
    }
    messages.insert(
        0,
        Message {
            role: Role::System,
            content: vec![ContentBlock::Text(TextBlock {
                text: block,
                cache_control: None,
            })],
        },
    );
}
