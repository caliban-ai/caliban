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
        thinking: thinking_from_request(req.thinking, req.effort),
        metadata: Some(NativeMetadata {
            user_id: req.metadata.user_id,
        }),
        stream,
        anthropic_version: None,
    }
}

/// Fallback thinking budget for an explicit `On` with no budget and no effort
/// signal to derive one from. Matches `Effort::Medium`'s Anthropic budget.
const DEFAULT_ON_BUDGET_TOKENS: u32 = 8_192;

/// Derive the Anthropic `thinking` block from the per-request
/// [`caliban_provider::ThinkingSetting`] plus the `Effort` hint (ticket #100).
///
/// - `Off` — omit the block entirely, even at high effort.
/// - `On(Some(b))` — enable with the explicit budget `b`.
/// - `On(None)` — enable with the effort-derived budget, or
///   [`DEFAULT_ON_BUDGET_TOKENS`] when effort gives no signal.
/// - `Auto` — legacy behavior: derive purely from `effort` (omit on
///   `Effort::Auto`/`None`).
fn thinking_from_request(
    setting: caliban_provider::ThinkingSetting,
    effort: Option<caliban_provider::Effort>,
) -> Option<NativeThinking> {
    use caliban_provider::ThinkingSetting;
    let enabled = |budget_tokens| NativeThinking {
        kind: "enabled".into(),
        budget_tokens,
    };
    let from_effort = || effort.and_then(caliban_provider::Effort::as_anthropic_budget);
    match setting {
        ThinkingSetting::Off => None,
        ThinkingSetting::On(Some(budget_tokens)) => Some(enabled(budget_tokens)),
        ThinkingSetting::On(None) => {
            Some(enabled(from_effort().unwrap_or(DEFAULT_ON_BUDGET_TOKENS)))
        }
        ThinkingSetting::Auto => from_effort().map(enabled),
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
                // BlobRef is session-local; resolve it to Base64 before
                // dispatching. We do a best-effort conversion to an
                // empty-data block which providers will reject — better than
                // silently dropping it.
                IrImageSource::BlobRef {
                    media_type,
                    sha256: _,
                } => NativeImageSource::Base64 {
                    media_type,
                    data: String::new(),
                },
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
            sha256: None,
            dims: None,
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

#[cfg(test)]
mod effort_plumbing {
    use super::*;
    use caliban_provider::{CompletionRequest, Effort, RequestMetadata};

    fn build_test_request() -> CompletionRequest {
        CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text(IrTextBlock {
                    text: "hi".into(),
                    cache_control: None,
                })],
            }],
            tools: vec![],
            tool_choice: caliban_provider::ToolChoice::default(),
            max_tokens: 256,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: caliban_provider::ThinkingSetting::Auto,
            effort: None,
            metadata: RequestMetadata::default(),
        }
    }

    #[test]
    fn effort_low_sets_thinking_budget_2048() {
        let mut req = build_test_request();
        req.effort = Some(Effort::Low);
        let native = ir_to_native_request(req, false);
        let thinking = native.thinking.expect("thinking block emitted");
        assert_eq!(thinking.kind, "enabled");
        assert_eq!(thinking.budget_tokens, 2_048);
    }

    #[test]
    fn effort_high_sets_thinking_budget_24576() {
        let mut req = build_test_request();
        req.effort = Some(Effort::High);
        let native = ir_to_native_request(req, false);
        let thinking = native.thinking.expect("thinking block emitted");
        assert_eq!(thinking.budget_tokens, 24_576);
    }

    #[test]
    fn effort_auto_omits_thinking_field() {
        let mut req = build_test_request();
        req.effort = Some(Effort::Auto);
        let native = ir_to_native_request(req, false);
        assert!(native.thinking.is_none(), "thinking field omitted on Auto");
    }

    #[test]
    fn explicit_on_budget_wins_over_effort() {
        let mut req = build_test_request();
        req.thinking = caliban_provider::ThinkingSetting::On(Some(1234));
        req.effort = Some(Effort::High);
        let native = ir_to_native_request(req, false);
        let thinking = native.thinking.expect("thinking block emitted");
        assert_eq!(
            thinking.budget_tokens, 1234,
            "explicit On(budget) takes precedence over effort"
        );
    }

    #[test]
    fn off_suppresses_thinking_even_at_high_effort() {
        let mut req = build_test_request();
        req.thinking = caliban_provider::ThinkingSetting::Off;
        req.effort = Some(Effort::High);
        let native = ir_to_native_request(req, false);
        assert!(
            native.thinking.is_none(),
            "Off must omit the thinking block regardless of effort"
        );
    }

    #[test]
    fn on_without_budget_uses_effort_budget() {
        let mut req = build_test_request();
        req.thinking = caliban_provider::ThinkingSetting::On(None);
        req.effort = Some(Effort::High);
        let native = ir_to_native_request(req, false);
        let thinking = native.thinking.expect("thinking block emitted");
        assert_eq!(
            thinking.budget_tokens, 24_576,
            "On(None) falls back to the effort-derived budget"
        );
    }

    #[test]
    fn on_without_budget_or_effort_uses_default() {
        let mut req = build_test_request();
        req.thinking = caliban_provider::ThinkingSetting::On(None);
        req.effort = Some(Effort::Auto);
        let native = ir_to_native_request(req, false);
        let thinking = native.thinking.expect("thinking block emitted");
        assert_eq!(
            thinking.budget_tokens, DEFAULT_ON_BUDGET_TOKENS,
            "On(None) with no effort signal falls back to a sane default budget"
        );
    }
}

#[cfg(test)]
mod tests {
    // Test code favors exhaustive `other => panic!` arms (clearer failure
    // messages than `unreachable!`) and a single broad coverage test, both of
    // which trip pedantic lints that are not meaningful for tests.
    #![allow(clippy::match_wildcard_for_single_variants)]
    #![allow(clippy::too_many_lines)]

    use super::*;
    use crate::schema::response::NativeUsage;
    use caliban_provider::{CompletionRequest, RequestMetadata};
    use serde_json::json;

    /// Build a minimal request with the given messages and tools. Tool-choice
    /// defaults to Auto; callers override as needed.
    fn req_with(messages: Vec<Message>, tools: Vec<IrTool>) -> CompletionRequest {
        CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            messages,
            tools,
            tool_choice: IrToolChoice::Auto,
            max_tokens: 256,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: caliban_provider::ThinkingSetting::Auto,
            effort: None,
            metadata: RequestMetadata::default(),
        }
    }

    fn sample_tool() -> IrTool {
        IrTool {
            name: "get_weather".into(),
            description: "Look up the weather".into(),
            input_schema: json!({"type": "object"}),
            cache_control: None,
        }
    }

    // ----- ir_to_native_request: system handling -----

    #[test]
    fn system_plain_text_joined_when_no_cache_control() {
        let req = req_with(
            vec![
                Message::system_text("a"),
                Message::system_text("b"),
                Message::user_text("hi"),
            ],
            vec![],
        );
        let native = ir_to_native_request(req, false);
        match native.system {
            Some(NativeSystem::Text(s)) => assert_eq!(s, "a\n\nb"),
            other => panic!("expected plain Text system, got {other:?}"),
        }
    }

    #[test]
    fn system_with_cache_control_becomes_blocks() {
        let system_msg = Message {
            role: Role::System,
            content: vec![ContentBlock::Text(IrTextBlock {
                text: "cached".into(),
                cache_control: Some(CacheControl::Ephemeral),
            })],
        };
        let req = req_with(vec![system_msg, Message::user_text("hi")], vec![]);
        let native = ir_to_native_request(req, false);
        match native.system {
            Some(NativeSystem::Blocks(blocks)) => {
                assert_eq!(blocks.len(), 1);
                assert_eq!(blocks[0].text, "cached");
                assert_eq!(blocks[0].cache_control, Some(NativeCacheControl::Ephemeral));
            }
            other => panic!("expected Blocks system, got {other:?}"),
        }
    }

    #[test]
    fn no_system_messages_yields_none() {
        let req = req_with(vec![Message::user_text("hi")], vec![]);
        let native = ir_to_native_request(req, false);
        assert!(native.system.is_none());
    }

    // ----- ir_to_native_request: tool_choice mapping -----

    #[test]
    fn tool_choice_none_when_tools_empty_regardless() {
        let mut req = req_with(vec![Message::user_text("hi")], vec![]);
        req.tool_choice = IrToolChoice::Any;
        let native = ir_to_native_request(req, false);
        assert!(native.tool_choice.is_none());
    }

    #[test]
    fn tool_choice_auto_with_tools() {
        let mut req = req_with(vec![Message::user_text("hi")], vec![sample_tool()]);
        req.tool_choice = IrToolChoice::Auto;
        let native = ir_to_native_request(req, false);
        assert_eq!(native.tool_choice, Some(NativeToolChoice::Auto));
    }

    #[test]
    fn tool_choice_any_with_tools() {
        let mut req = req_with(vec![Message::user_text("hi")], vec![sample_tool()]);
        req.tool_choice = IrToolChoice::Any;
        let native = ir_to_native_request(req, false);
        assert_eq!(native.tool_choice, Some(NativeToolChoice::Any));
    }

    #[test]
    fn tool_choice_specific_with_tools() {
        let mut req = req_with(vec![Message::user_text("hi")], vec![sample_tool()]);
        req.tool_choice = IrToolChoice::Specific {
            name: "get_weather".into(),
        };
        let native = ir_to_native_request(req, false);
        assert_eq!(
            native.tool_choice,
            Some(NativeToolChoice::Tool {
                name: "get_weather".into()
            })
        );
    }

    #[test]
    fn tool_choice_none_variant_yields_none_even_with_tools() {
        let mut req = req_with(vec![Message::user_text("hi")], vec![sample_tool()]);
        req.tool_choice = IrToolChoice::None;
        let native = ir_to_native_request(req, false);
        assert!(native.tool_choice.is_none());
    }

    #[test]
    fn tool_with_cache_control_mapped() {
        let mut tool = sample_tool();
        tool.cache_control = Some(CacheControl::Ephemeral);
        let req = req_with(vec![Message::user_text("hi")], vec![tool]);
        let native = ir_to_native_request(req, false);
        assert_eq!(native.tools.len(), 1);
        assert_eq!(native.tools[0].name, "get_weather");
        assert_eq!(
            native.tools[0].cache_control,
            Some(NativeCacheControl::Ephemeral)
        );
    }

    // ----- ir_content_block_to_native (driven via messages) -----

    #[test]
    fn assistant_message_blocks_converted() {
        let assistant = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text(IrTextBlock {
                    text: "thought".into(),
                    cache_control: Some(CacheControl::Ephemeral),
                }),
                ContentBlock::Image(IrImageBlock {
                    source: IrImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "aGVsbG8=".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
                ContentBlock::ToolUse(IrToolUseBlock {
                    id: "tu_1".into(),
                    name: "do_thing".into(),
                    input: json!({"k": "v"}),
                }),
            ],
        };
        let req = req_with(vec![assistant], vec![]);
        let native = ir_to_native_request(req, false);
        assert_eq!(native.messages.len(), 1);
        assert_eq!(native.messages[0].role, "assistant");
        let blocks = match &native.messages[0].content {
            NativeContent::Blocks(b) => b,
            other => panic!("expected Blocks, got {other:?}"),
        };
        assert_eq!(blocks.len(), 3);
        match &blocks[0] {
            NativeContentBlock::Text(t) => {
                assert_eq!(t.text, "thought");
                assert_eq!(t.cache_control, Some(NativeCacheControl::Ephemeral));
            }
            other => panic!("expected Text, got {other:?}"),
        }
        match &blocks[1] {
            NativeContentBlock::Image(i) => match &i.source {
                NativeImageSource::Base64 { media_type, data } => {
                    assert_eq!(media_type, "image/png");
                    assert_eq!(data, "aGVsbG8=");
                }
                other => panic!("expected Base64 source, got {other:?}"),
            },
            other => panic!("expected Image, got {other:?}"),
        }
        match &blocks[2] {
            NativeContentBlock::ToolUse(tu) => {
                assert_eq!(tu.id, "tu_1");
                assert_eq!(tu.name, "do_thing");
                assert_eq!(tu.input, json!({"k": "v"}));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn user_message_tool_result_image_variants_and_thinking() {
        let user = Message {
            role: Role::User,
            content: vec![
                ContentBlock::ToolResult(IrToolResultBlock {
                    tool_use_id: "tu_1".into(),
                    content: vec![ContentBlock::Text(IrTextBlock {
                        text: "nested".into(),
                        cache_control: None,
                    })],
                    is_error: true,
                }),
                ContentBlock::Image(IrImageBlock {
                    source: IrImageSource::Url {
                        url: "https://example.com/a.png".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
                ContentBlock::Image(IrImageBlock {
                    source: IrImageSource::BlobRef {
                        sha256: "deadbeef".into(),
                        media_type: "image/jpeg".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
                ContentBlock::Thinking(IrThinkingBlock {
                    thinking: "hmm".into(),
                    signature: Some("sig".into()),
                }),
            ],
        };
        let req = req_with(vec![user], vec![]);
        let native = ir_to_native_request(req, false);
        assert_eq!(native.messages[0].role, "user");
        let blocks = match &native.messages[0].content {
            NativeContent::Blocks(b) => b,
            other => panic!("expected Blocks, got {other:?}"),
        };
        assert_eq!(blocks.len(), 4);

        match &blocks[0] {
            NativeContentBlock::ToolResult(tr) => {
                assert_eq!(tr.tool_use_id, "tu_1");
                assert!(tr.is_error);
                match &tr.content {
                    NativeContent::Blocks(inner) => {
                        assert_eq!(inner.len(), 1);
                        match &inner[0] {
                            NativeContentBlock::Text(t) => assert_eq!(t.text, "nested"),
                            other => panic!("expected nested Text, got {other:?}"),
                        }
                    }
                    other => panic!("expected nested Blocks, got {other:?}"),
                }
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        match &blocks[1] {
            NativeContentBlock::Image(i) => match &i.source {
                NativeImageSource::Url { url } => {
                    assert_eq!(url, "https://example.com/a.png");
                }
                other => panic!("expected Url source, got {other:?}"),
            },
            other => panic!("expected Image, got {other:?}"),
        }
        // BlobRef best-effort converts to empty-data Base64.
        match &blocks[2] {
            NativeContentBlock::Image(i) => match &i.source {
                NativeImageSource::Base64 { media_type, data } => {
                    assert_eq!(media_type, "image/jpeg");
                    assert_eq!(data, "");
                }
                other => panic!("expected Base64 source from BlobRef, got {other:?}"),
            },
            other => panic!("expected Image, got {other:?}"),
        }
        match &blocks[3] {
            NativeContentBlock::Thinking(t) => {
                assert_eq!(t.thinking, "hmm");
                assert_eq!(t.signature, Some("sig".into()));
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    // ----- native_response_to_ir -----

    fn base_response(
        content: Vec<NativeContentBlock>,
        stop_reason: NativeStopReason,
        usage: NativeUsage,
    ) -> NativeResponse {
        NativeResponse {
            id: "msg_1".into(),
            model: "claude-sonnet-4-6".into(),
            role: "assistant".into(),
            kind: "message".into(),
            content,
            stop_reason,
            stop_sequence: None,
            usage,
        }
    }

    fn simple_usage() -> NativeUsage {
        NativeUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    #[test]
    fn response_stop_reason_mapping() {
        let cases = [
            (NativeStopReason::MaxTokens, StopReason::MaxTokens),
            (NativeStopReason::StopSequence, StopReason::StopSequence),
            (NativeStopReason::ToolUse, StopReason::ToolUse),
            (NativeStopReason::Refusal, StopReason::Refusal),
            (NativeStopReason::EndTurn, StopReason::EndTurn),
            (NativeStopReason::PauseTurn, StopReason::EndTurn),
        ];
        for (native_sr, expected) in cases {
            let resp = base_response(
                vec![NativeContentBlock::Text(NativeTextBlock {
                    text: "ok".into(),
                    cache_control: None,
                })],
                native_sr,
                simple_usage(),
            );
            let ir = native_response_to_ir(resp).expect("convert");
            assert_eq!(ir.stop_reason, expected, "for {native_sr:?}");
        }
    }

    #[test]
    fn response_usage_sums_cache_fields() {
        let resp = base_response(
            vec![NativeContentBlock::Text(NativeTextBlock {
                text: "ok".into(),
                cache_control: None,
            })],
            NativeStopReason::EndTurn,
            NativeUsage {
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_input_tokens: Some(30),
                cache_read_input_tokens: Some(7),
            },
        );
        let ir = native_response_to_ir(resp).expect("convert");
        assert_eq!(ir.usage.input_tokens, 137);
        assert_eq!(ir.usage.output_tokens, 20);
        assert_eq!(ir.usage.cache_creation_input_tokens, Some(30));
        assert_eq!(ir.usage.cache_read_input_tokens, Some(7));
    }

    #[test]
    fn response_usage_with_no_cache_fields() {
        let resp = base_response(
            vec![NativeContentBlock::Text(NativeTextBlock {
                text: "ok".into(),
                cache_control: None,
            })],
            NativeStopReason::EndTurn,
            simple_usage(),
        );
        let ir = native_response_to_ir(resp).expect("convert");
        assert_eq!(ir.usage.input_tokens, 10);
        assert_eq!(ir.usage.cache_creation_input_tokens, None);
        assert_eq!(ir.usage.cache_read_input_tokens, None);
    }

    #[test]
    fn response_metadata_preserved() {
        let resp = base_response(vec![], NativeStopReason::EndTurn, simple_usage());
        let ir = native_response_to_ir(resp).expect("convert");
        assert_eq!(ir.id, "msg_1");
        assert_eq!(ir.model, "claude-sonnet-4-6");
        assert_eq!(ir.message.role, Role::Assistant);
    }

    // ----- native_block_to_ir (via native_response_to_ir) -----

    #[test]
    fn native_block_to_ir_all_variants() {
        let content = vec![
            NativeContentBlock::Text(NativeTextBlock {
                text: "txt".into(),
                cache_control: Some(NativeCacheControl::Ephemeral),
            }),
            NativeContentBlock::Image(NativeImageBlock {
                source: NativeImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: "ZGF0YQ==".into(),
                },
                cache_control: None,
            }),
            NativeContentBlock::Image(NativeImageBlock {
                source: NativeImageSource::Url {
                    url: "https://example.com/i.png".into(),
                },
                cache_control: None,
            }),
            NativeContentBlock::ToolUse(NativeToolUseBlock {
                id: "tu_9".into(),
                name: "calc".into(),
                input: json!({"x": 1}),
            }),
            // ToolResult with NativeContent::Text variant.
            NativeContentBlock::ToolResult(NativeToolResultBlock {
                tool_use_id: "tu_text".into(),
                content: NativeContent::Text("plain result".into()),
                is_error: false,
            }),
            // ToolResult with NativeContent::Blocks variant (recursion).
            NativeContentBlock::ToolResult(NativeToolResultBlock {
                tool_use_id: "tu_blocks".into(),
                content: NativeContent::Blocks(vec![NativeContentBlock::Text(NativeTextBlock {
                    text: "block result".into(),
                    cache_control: None,
                })]),
                is_error: true,
            }),
            NativeContentBlock::Thinking(NativeThinkingBlock {
                thinking: "reasoning".into(),
                signature: Some("sig9".into()),
            }),
            NativeContentBlock::RedactedThinking {
                data: "redacted-blob".into(),
            },
        ];
        let resp = base_response(content, NativeStopReason::EndTurn, simple_usage());
        let ir = native_response_to_ir(resp).expect("convert");
        let blocks = &ir.message.content;
        assert_eq!(blocks.len(), 8);

        match &blocks[0] {
            ContentBlock::Text(t) => {
                assert_eq!(t.text, "txt");
                assert_eq!(t.cache_control, Some(CacheControl::Ephemeral));
            }
            other => panic!("expected Text, got {other:?}"),
        }
        match &blocks[1] {
            ContentBlock::Image(i) => {
                assert!(matches!(
                    &i.source,
                    IrImageSource::Base64 { media_type, data }
                        if media_type == "image/png" && data == "ZGF0YQ=="
                ));
                assert_eq!(i.sha256, None);
                assert_eq!(i.dims, None);
            }
            other => panic!("expected Image, got {other:?}"),
        }
        match &blocks[2] {
            ContentBlock::Image(i) => assert!(matches!(
                &i.source,
                IrImageSource::Url { url } if url == "https://example.com/i.png"
            )),
            other => panic!("expected Image, got {other:?}"),
        }
        match &blocks[3] {
            ContentBlock::ToolUse(tu) => {
                assert_eq!(tu.id, "tu_9");
                assert_eq!(tu.name, "calc");
                assert_eq!(tu.input, json!({"x": 1}));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        match &blocks[4] {
            ContentBlock::ToolResult(tr) => {
                assert_eq!(tr.tool_use_id, "tu_text");
                assert!(!tr.is_error);
                assert_eq!(tr.content.len(), 1);
                match &tr.content[0] {
                    ContentBlock::Text(t) => assert_eq!(t.text, "plain result"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        match &blocks[5] {
            ContentBlock::ToolResult(tr) => {
                assert_eq!(tr.tool_use_id, "tu_blocks");
                assert!(tr.is_error);
                assert_eq!(tr.content.len(), 1);
                match &tr.content[0] {
                    ContentBlock::Text(t) => assert_eq!(t.text, "block result"),
                    other => panic!("expected Text, got {other:?}"),
                }
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        match &blocks[6] {
            ContentBlock::Thinking(t) => {
                assert_eq!(t.thinking, "reasoning");
                assert_eq!(t.signature, Some("sig9".into()));
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
        // RedactedThinking -> Thinking { thinking: "", signature: Some(data) }.
        match &blocks[7] {
            ContentBlock::Thinking(t) => {
                assert_eq!(t.thinking, "");
                assert_eq!(t.signature, Some("redacted-blob".into()));
            }
            other => panic!("expected Thinking from RedactedThinking, got {other:?}"),
        }
    }
}
