//! IR ↔ Anthropic native conversions for request and response.

use caliban_provider::{
    CacheControl, ContentBlock, ImageBlock as IrImageBlock, ImageSource as IrImageSource, Message,
    Result, Role, StopReason, TextBlock as IrTextBlock, ThinkingBlock as IrThinkingBlock,
    Tool as IrTool, ToolChoice as IrToolChoice, ToolResultBlock as IrToolResultBlock,
    ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
};

use crate::schema::request::{
    NativeCacheControl, NativeContent, NativeContentBlock, NativeImageBlock, NativeImageSource,
    NativeMessage, NativeMetadata, NativeRequest, NativeSystem, NativeTextBlock, NativeThinking,
    NativeThinkingBlock, NativeTool, NativeToolChoice, NativeToolResultBlock, NativeToolUseBlock,
};
use crate::schema::response::{NativeResponse, NativeStopReason};

/// Convert an IR `CompletionRequest` to the Anthropic wire format.
///
/// # Panics
///
/// Never panics in practice — the internal `messages.next()` is always safe
/// because it is guarded by a preceding `peek()`.
pub fn ir_to_native_request(
    req: caliban_provider::CompletionRequest,
    stream: bool,
) -> NativeRequest {
    // Split off leading System messages
    let mut messages = req.messages.into_iter().peekable();
    let mut system_blocks: Vec<NativeTextBlock> = Vec::new();
    while let Some(m) = messages.peek() {
        if m.role != Role::System {
            break;
        }
        let m = messages.next().expect("peeked");
        for cb in m.content {
            if let ContentBlock::Text(tb) = cb {
                system_blocks.push(NativeTextBlock {
                    text: tb.text,
                    cache_control: tb.cache_control.map(|_| NativeCacheControl::Ephemeral),
                });
            }
        }
    }
    let system = if system_blocks.is_empty() {
        None
    } else if system_blocks.iter().all(|b| b.cache_control.is_none()) {
        Some(NativeSystem::Text(
            system_blocks
                .into_iter()
                .map(|b| b.text)
                .collect::<Vec<_>>()
                .join("\n\n"),
        ))
    } else {
        Some(NativeSystem::Blocks(system_blocks))
    };

    let native_messages: Vec<NativeMessage> = messages
        .map(|m| NativeMessage {
            role: match m.role {
                Role::User => "user".into(),
                Role::Assistant => "assistant".into(),
                Role::System => unreachable!("System filtered above"),
            },
            content: NativeContent::Blocks(
                m.content
                    .into_iter()
                    .map(ir_content_block_to_native)
                    .collect(),
            ),
        })
        .collect();

    let no_tools = req.tools.is_empty();
    NativeRequest {
        model: req.model,
        messages: native_messages,
        system,
        tools: req.tools.into_iter().map(ir_tool_to_native).collect(),
        tool_choice: ir_tool_choice_to_native(req.tool_choice, no_tools),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        stop_sequences: req.stop_sequences,
        thinking: req.thinking.map(|t| NativeThinking {
            kind: "enabled".into(),
            budget_tokens: t.budget_tokens,
        }),
        metadata: Some(NativeMetadata {
            user_id: req.metadata.user_id,
        }),
        stream,
        anthropic_version: None,
    }
}

fn ir_content_block_to_native(b: ContentBlock) -> NativeContentBlock {
    match b {
        ContentBlock::Text(t) => NativeContentBlock::Text(NativeTextBlock {
            text: t.text,
            cache_control: t.cache_control.map(|_| NativeCacheControl::Ephemeral),
        }),
        ContentBlock::Image(i) => NativeContentBlock::Image(NativeImageBlock {
            source: match i.source {
                IrImageSource::Base64 { media_type, data } => {
                    NativeImageSource::Base64 { media_type, data }
                }
                // ImageSource::Url is a struct variant: { url: String }
                IrImageSource::Url { url } => NativeImageSource::Url { url },
            },
            cache_control: i.cache_control.map(|_| NativeCacheControl::Ephemeral),
        }),
        ContentBlock::ToolUse(tu) => NativeContentBlock::ToolUse(NativeToolUseBlock {
            id: tu.id,
            name: tu.name,
            input: tu.input,
        }),
        ContentBlock::ToolResult(tr) => NativeContentBlock::ToolResult(NativeToolResultBlock {
            tool_use_id: tr.tool_use_id,
            content: NativeContent::Blocks(
                tr.content
                    .into_iter()
                    .map(ir_content_block_to_native)
                    .collect(),
            ),
            is_error: tr.is_error,
        }),
        ContentBlock::Thinking(t) => NativeContentBlock::Thinking(NativeThinkingBlock {
            thinking: t.thinking,
            signature: t.signature,
        }),
    }
}

fn ir_tool_to_native(t: IrTool) -> NativeTool {
    NativeTool {
        name: t.name,
        description: t.description,
        input_schema: t.input_schema,
        cache_control: t.cache_control.map(|_| NativeCacheControl::Ephemeral),
    }
}

fn ir_tool_choice_to_native(c: IrToolChoice, no_tools: bool) -> Option<NativeToolChoice> {
    if no_tools {
        return None;
    }
    match c {
        IrToolChoice::Auto => Some(NativeToolChoice::Auto),
        IrToolChoice::Any => Some(NativeToolChoice::Any),
        IrToolChoice::Specific { name } => Some(NativeToolChoice::Tool { name }),
        IrToolChoice::None => None,
    }
}

/// Convert a native Anthropic response to the IR `CompletionResponse`.
///
/// # Errors
///
/// Returns `Err` if any content block cannot be converted (e.g. unknown block type).
pub fn native_response_to_ir(r: NativeResponse) -> Result<caliban_provider::CompletionResponse> {
    let content = r
        .content
        .into_iter()
        .map(native_block_to_ir)
        .collect::<Result<Vec<_>>>()?;
    Ok(caliban_provider::CompletionResponse {
        id: r.id,
        model: r.model,
        message: Message {
            role: Role::Assistant,
            content,
        },
        stop_reason: match r.stop_reason {
            NativeStopReason::MaxTokens => StopReason::MaxTokens,
            NativeStopReason::StopSequence => StopReason::StopSequence,
            NativeStopReason::ToolUse => StopReason::ToolUse,
            NativeStopReason::Refusal => StopReason::Refusal,
            // PauseTurn (Bedrock-specific) maps to EndTurn at the IR level.
            NativeStopReason::EndTurn | NativeStopReason::PauseTurn => StopReason::EndTurn,
        },
        stop_sequence: r.stop_sequence,
        usage: IrUsage {
            // Normalize to the OpenAI convention: input_tokens is the TOTAL
            // prompt size (including any cached portion). Anthropic reports
            // these three counters disjointly, so we sum them here. The
            // separated cache counters are preserved unchanged.
            input_tokens: r.usage.input_tokens
                + r.usage.cache_creation_input_tokens.unwrap_or(0)
                + r.usage.cache_read_input_tokens.unwrap_or(0),
            output_tokens: r.usage.output_tokens,
            cache_creation_input_tokens: r.usage.cache_creation_input_tokens,
            cache_read_input_tokens: r.usage.cache_read_input_tokens,
        },
    })
}

fn native_block_to_ir(b: NativeContentBlock) -> Result<ContentBlock> {
    Ok(match b {
        NativeContentBlock::Text(t) => ContentBlock::Text(IrTextBlock {
            text: t.text,
            cache_control: t.cache_control.map(|_| CacheControl::Ephemeral),
        }),
        NativeContentBlock::Image(i) => ContentBlock::Image(IrImageBlock {
            source: match i.source {
                NativeImageSource::Base64 { media_type, data } => {
                    IrImageSource::Base64 { media_type, data }
                }
                // NativeImageSource::Url is a struct variant: { url: String }
                NativeImageSource::Url { url } => IrImageSource::Url { url },
            },
            cache_control: i.cache_control.map(|_| CacheControl::Ephemeral),
        }),
        NativeContentBlock::ToolUse(tu) => ContentBlock::ToolUse(IrToolUseBlock {
            id: tu.id,
            name: tu.name,
            input: tu.input,
        }),
        NativeContentBlock::ToolResult(tr) => ContentBlock::ToolResult(IrToolResultBlock {
            tool_use_id: tr.tool_use_id,
            content: match tr.content {
                NativeContent::Text(s) => vec![ContentBlock::Text(IrTextBlock {
                    text: s,
                    cache_control: None,
                })],
                NativeContent::Blocks(bs) => bs
                    .into_iter()
                    .map(native_block_to_ir)
                    .collect::<Result<Vec<_>>>()?,
            },
            is_error: tr.is_error,
        }),
        NativeContentBlock::Thinking(t) => ContentBlock::Thinking(IrThinkingBlock {
            thinking: t.thinking,
            signature: t.signature,
        }),
        NativeContentBlock::RedactedThinking { data } => ContentBlock::Thinking(IrThinkingBlock {
            thinking: String::new(),
            signature: Some(data),
        }),
    })
}
