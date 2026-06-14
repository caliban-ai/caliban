//! IR ↔ Ollama native conversions for request and response.

use caliban_provider::{
    ContentBlock, Error, ImageSource as IrImageSource, Message, Result, Role, StopReason,
    TextBlock as IrTextBlock, ThinkingBlock as IrThinkingBlock, Tool as IrTool,
    ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
};

use crate::schema::request::{
    NativeFunctionCall, NativeMessage, NativeOptions, NativeRequest, NativeTool, NativeToolCall,
    NativeToolFunction,
};
use crate::schema::response::NativeResponse;

/// Convert a caliban IR `CompletionRequest` to an Ollama `NativeRequest`.
///
/// # Errors
///
/// Returns `Err(Error::InvalidRequest)` if the request contains a URL-sourced image,
/// since Ollama only supports base64-encoded images.
///
/// # Panics
///
/// Cannot panic in practice; the `expect` in the system-message collection loop is
/// guarded by a preceding `peek`.
#[allow(clippy::too_many_lines)]
pub fn ir_to_native_request(
    req: caliban_provider::CompletionRequest,
    stream: bool,
) -> Result<NativeRequest> {
    let mut messages_iter = req.messages.into_iter().peekable();

    // Collect leading System messages into one system message.
    let mut system_texts: Vec<String> = Vec::new();
    while let Some(m) = messages_iter.peek() {
        if m.role != Role::System {
            break;
        }
        let m = messages_iter.next().expect("just peeked");
        for cb in m.content {
            if let ContentBlock::Text(tb) = cb {
                system_texts.push(tb.text);
            }
        }
    }

    let mut native_messages: Vec<NativeMessage> = Vec::new();

    // Prepend a single system message if any leading system content existed.
    if !system_texts.is_empty() {
        native_messages.push(NativeMessage {
            role: "system".into(),
            content: system_texts.join("\n\n"),
            thinking: None,
            images: Vec::new(),
            tool_calls: Vec::new(),
        });
    }

    // Convert remaining User/Assistant messages.
    for msg in messages_iter {
        match msg.role {
            Role::System => {
                // System messages appearing after non-system messages are validated out by
                // CompletionRequest::validate(), but handle gracefully by concatenating.
                let text = msg
                    .content
                    .into_iter()
                    .filter_map(|cb| {
                        if let ContentBlock::Text(tb) = cb {
                            Some(tb.text)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                native_messages.push(NativeMessage {
                    role: "system".into(),
                    content: text,
                    thinking: None,
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                });
            }
            Role::User => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut images: Vec<String> = Vec::new();
                let mut tool_result_msgs: Vec<NativeMessage> = Vec::new();

                for cb in msg.content {
                    match cb {
                        ContentBlock::Text(tb) => {
                            text_parts.push(tb.text);
                        }
                        ContentBlock::Image(img) => match img.source {
                            IrImageSource::Base64 { data, .. } => {
                                images.push(data);
                            }
                            IrImageSource::Url { .. } => {
                                return Err(Error::InvalidRequest(
                                    "Ollama only supports base64 images".into(),
                                ));
                            }
                            IrImageSource::BlobRef { .. } => {
                                return Err(Error::InvalidRequest(
                                    "BlobRef image source must be resolved before \
                                     dispatch; got an unresolved session blob"
                                        .into(),
                                ));
                            }
                        },
                        ContentBlock::ToolResult(tr) => {
                            // Ollama tool results: {role:"tool", content: <text>}.
                            // No tool_call_id correlation.
                            let content_text = tr
                                .content
                                .into_iter()
                                .filter_map(|b| {
                                    if let ContentBlock::Text(t) = b {
                                        Some(t.text)
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            tool_result_msgs.push(NativeMessage {
                                role: "tool".into(),
                                content: content_text,
                                thinking: None,
                                images: Vec::new(),
                                tool_calls: Vec::new(),
                            });
                        }
                        // Thinking and unexpected ToolUse in User messages are dropped.
                        ContentBlock::Thinking(_) | ContentBlock::ToolUse(_) => {}
                    }
                }

                // Emit a user message if there was any text/image content.
                if !text_parts.is_empty() || !images.is_empty() {
                    native_messages.push(NativeMessage {
                        role: "user".into(),
                        content: text_parts.join(""),
                        thinking: None,
                        images,
                        tool_calls: Vec::new(),
                    });
                }

                // Append tool result messages after the user message.
                native_messages.extend(tool_result_msgs);
            }
            Role::Assistant => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<NativeToolCall> = Vec::new();

                for cb in msg.content {
                    match cb {
                        ContentBlock::Text(tb) => {
                            text_parts.push(tb.text);
                        }
                        ContentBlock::Thinking(t) => {
                            thinking_parts.push(t.thinking);
                        }
                        ContentBlock::ToolUse(tu) => {
                            // Ollama arguments is a JSON Value object, NOT a string.
                            // Preserve the IR call id so multi-turn correlation
                            // round-trips faithfully when the model produced one.
                            let id = if tu.id.starts_with("tool_") {
                                None
                            } else {
                                Some(tu.id)
                            };
                            tool_calls.push(NativeToolCall {
                                id,
                                function: NativeFunctionCall {
                                    name: tu.name,
                                    arguments: tu.input,
                                },
                            });
                        }
                        // Image and unexpected ToolResult in assistant messages are dropped.
                        ContentBlock::Image(_) | ContentBlock::ToolResult(_) => {}
                    }
                }

                let thinking = if thinking_parts.is_empty() {
                    None
                } else {
                    Some(thinking_parts.join(""))
                };

                native_messages.push(NativeMessage {
                    role: "assistant".into(),
                    content: text_parts.join(""),
                    thinking,
                    images: Vec::new(),
                    tool_calls,
                });
            }
        }
    }

    let tools: Vec<NativeTool> = req
        .tools
        .into_iter()
        .map(|t: IrTool| NativeTool {
            kind: "function".into(),
            function: NativeToolFunction {
                name: t.name,
                description: t.description,
                parameters: t.input_schema,
            },
        })
        .collect();

    let options = NativeOptions {
        num_predict: Some(req.max_tokens),
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        stop: req.stop_sequences,
    };

    Ok(NativeRequest {
        model: req.model,
        messages: native_messages,
        tools,
        stream,
        format: None,
        options,
        keep_alive: None,
    })
}

/// Convert an Ollama `NativeResponse` to a caliban `CompletionResponse`.
///
/// # Errors
///
/// This function currently does not fail, but returns `Result` for API consistency.
pub fn native_response_to_ir(r: NativeResponse) -> Result<caliban_provider::CompletionResponse> {
    let msg = r.message;
    let has_tool_calls = !msg.tool_calls.is_empty();

    let mut content_blocks: Vec<caliban_provider::ContentBlock> = Vec::new();

    // Map reasoning content (qwen3.5 and similar) into a Thinking block.
    // Emit it before the text block so the IR reflects the order the model
    // produced it (reasoning first, then the final answer).
    if let Some(thinking) = msg.thinking.filter(|t| !t.is_empty()) {
        content_blocks.push(caliban_provider::ContentBlock::Thinking(IrThinkingBlock {
            thinking,
            signature: None,
        }));
    }

    // Map text content.
    if !msg.content.is_empty() {
        content_blocks.push(caliban_provider::ContentBlock::Text(IrTextBlock {
            text: msg.content,
            cache_control: None,
        }));
    }

    // Map tool calls, preserving the upstream id when present.
    for (idx, tc) in msg.tool_calls.into_iter().enumerate() {
        let id = tc.id.unwrap_or_else(|| format!("tool_{idx}"));
        content_blocks.push(caliban_provider::ContentBlock::ToolUse(IrToolUseBlock {
            id,
            name: tc.function.name,
            input: tc.function.arguments,
        }));
    }

    // Ollama reports `done_reason: "stop"` even on tool-calling turns; the
    // presence of tool_calls is the authoritative signal that the agent
    // loop must continue.
    let stop_reason = if has_tool_calls {
        StopReason::ToolUse
    } else {
        map_done_reason(r.done_reason.as_deref())
    };

    Ok(caliban_provider::CompletionResponse {
        id: String::new(),
        model: r.model,
        message: Message {
            role: Role::Assistant,
            content: content_blocks,
        },
        stop_reason,
        stop_sequence: None,
        usage: IrUsage {
            input_tokens: r.prompt_eval_count,
            output_tokens: r.eval_count,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    })
}

/// Map Ollama's `done_reason` string to the IR `StopReason`.
///
/// Note: `done_reason: "stop"` is **not** a reliable end-of-turn signal when
/// the chunk also carries `tool_calls` — Ollama uses `"stop"` in both cases.
/// Callers that have access to the full message must check for `tool_calls`
/// and prefer `StopReason::ToolUse` in that case; this helper only maps the
/// raw string.
pub(crate) fn map_done_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("length") => StopReason::MaxTokens,
        Some("tool_calls") => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use caliban_provider::{
        CompletionRequest, ImageBlock, ImageSource, RequestMetadata, Tool, ToolChoice,
        ToolResultBlock, ToolUseBlock,
    };
    use serde_json::{Value, json};

    use crate::schema::response::NativeResponse;

    /// Build a minimal `CompletionRequest` struct literal, bypassing
    /// `validate()` so we can exercise edge cases (e.g. non-leading System).
    fn req_with(messages: Vec<Message>) -> CompletionRequest {
        CompletionRequest {
            model: "test-model".into(),
            messages,
            tools: vec![],
            tool_choice: ToolChoice::Auto,
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

    fn text_block(s: &str) -> ContentBlock {
        ContentBlock::Text(IrTextBlock {
            text: s.into(),
            cache_control: None,
        })
    }

    fn sys_msg(s: &str) -> Message {
        Message {
            role: Role::System,
            content: vec![text_block(s)],
        }
    }

    fn native_resp(message: NativeMessage, done_reason: Option<&str>) -> NativeResponse {
        NativeResponse {
            model: "test-model".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            message,
            done: true,
            done_reason: done_reason.map(Into::into),
            prompt_eval_count: 0,
            eval_count: 0,
        }
    }

    fn empty_native_msg() -> NativeMessage {
        NativeMessage {
            role: "assistant".into(),
            content: String::new(),
            thinking: None,
            images: Vec::new(),
            tool_calls: Vec::new(),
        }
    }

    // ---- ir_to_native_request ----

    #[test]
    fn leading_system_messages_joined() {
        let req = req_with(vec![sys_msg("a"), sys_msg("b"), Message::user_text("hi")]);
        let native = ir_to_native_request(req, false).unwrap();
        assert_eq!(native.messages[0].role, "system");
        assert_eq!(native.messages[0].content, "a\n\nb");
        assert_eq!(native.messages[1].role, "user");
        assert_eq!(native.messages[1].content, "hi");
        assert!(!native.stream);
        assert_eq!(native.model, "test-model");
    }

    #[test]
    fn non_leading_system_appended() {
        // User then System: System comes after a non-system message. Built as a
        // struct literal to bypass validate(), which would reject this ordering.
        let req = req_with(vec![Message::user_text("hi"), sys_msg("late instruction")]);
        let native = ir_to_native_request(req, false).unwrap();
        assert_eq!(native.messages[0].role, "user");
        assert_eq!(native.messages[1].role, "system");
        assert_eq!(native.messages[1].content, "late instruction");
    }

    #[test]
    fn non_leading_system_multiple_text_blocks_joined() {
        let user_then_sys = vec![
            Message::user_text("hi"),
            Message {
                role: Role::System,
                content: vec![text_block("x"), text_block("y")],
            },
        ];
        let native = ir_to_native_request(req_with(user_then_sys), false).unwrap();
        assert_eq!(native.messages[1].role, "system");
        assert_eq!(native.messages[1].content, "x\n\ny");
    }

    #[test]
    fn user_text_and_base64_image() {
        let img = ContentBlock::Image(ImageBlock {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "AAAAdata".into(),
            },
            cache_control: None,
            sha256: None,
            dims: None,
        });
        let user = Message {
            role: Role::User,
            content: vec![text_block("look:"), img],
        };
        let native = ir_to_native_request(req_with(vec![user]), false).unwrap();
        let m = &native.messages[0];
        assert_eq!(m.role, "user");
        assert_eq!(m.content, "look:");
        // images carry only the base64 data, no MIME prefix.
        assert_eq!(m.images, vec!["AAAAdata".to_string()]);
    }

    #[test]
    fn user_image_url_errors() {
        let img = ContentBlock::Image(ImageBlock {
            source: ImageSource::Url {
                url: "https://example.com/x.png".into(),
            },
            cache_control: None,
            sha256: None,
            dims: None,
        });
        let user = Message {
            role: Role::User,
            content: vec![img],
        };
        let err = ir_to_native_request(req_with(vec![user]), false).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
    }

    #[test]
    fn user_image_blobref_errors() {
        let img = ContentBlock::Image(ImageBlock {
            source: ImageSource::BlobRef {
                sha256: "deadbeef".into(),
                media_type: "image/png".into(),
            },
            cache_control: None,
            sha256: None,
            dims: None,
        });
        let user = Message {
            role: Role::User,
            content: vec![img],
        };
        let err = ir_to_native_request(req_with(vec![user]), false).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
    }

    #[test]
    fn user_tool_result_becomes_tool_message() {
        let tr = ContentBlock::ToolResult(ToolResultBlock {
            tool_use_id: "call_1".into(),
            content: vec![text_block("part1"), text_block("part2")],
            is_error: false,
        });
        let user = Message {
            role: Role::User,
            content: vec![text_block("here:"), tr],
        };
        let native = ir_to_native_request(req_with(vec![user]), false).unwrap();
        // First the user message (from the text), then the tool message.
        assert_eq!(native.messages[0].role, "user");
        assert_eq!(native.messages[0].content, "here:");
        assert_eq!(native.messages[1].role, "tool");
        // Nested Text blocks are joined with newline.
        assert_eq!(native.messages[1].content, "part1\npart2");
    }

    #[test]
    fn user_with_only_tool_result_emits_no_user_message() {
        let tr = ContentBlock::ToolResult(ToolResultBlock {
            tool_use_id: "call_1".into(),
            content: vec![text_block("only-result")],
            is_error: false,
        });
        let user = Message {
            role: Role::User,
            content: vec![tr],
        };
        let native = ir_to_native_request(req_with(vec![user]), false).unwrap();
        assert_eq!(native.messages.len(), 1);
        assert_eq!(native.messages[0].role, "tool");
        assert_eq!(native.messages[0].content, "only-result");
    }

    #[test]
    fn assistant_text_thinking_and_tool_use() {
        let assistant = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking(IrThinkingBlock {
                    thinking: "step1".into(),
                    signature: None,
                }),
                ContentBlock::Thinking(IrThinkingBlock {
                    thinking: "step2".into(),
                    signature: None,
                }),
                text_block("answer"),
                // id starting with "tool_" is treated as synthesized -> None.
                ContentBlock::ToolUse(ToolUseBlock {
                    id: "tool_0".into(),
                    name: "synth".into(),
                    input: json!({"a": 1}),
                }),
                // a real upstream id is preserved.
                ContentBlock::ToolUse(ToolUseBlock {
                    id: "call_real".into(),
                    name: "real".into(),
                    input: json!({"b": 2}),
                }),
                // Image and ToolResult in assistant messages are dropped.
                ContentBlock::Image(ImageBlock {
                    source: ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "img".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
                ContentBlock::ToolResult(ToolResultBlock {
                    tool_use_id: "x".into(),
                    content: vec![],
                    is_error: false,
                }),
            ],
        };
        let native = ir_to_native_request(req_with(vec![assistant]), false).unwrap();
        let m = &native.messages[0];
        assert_eq!(m.role, "assistant");
        assert_eq!(m.content, "answer");
        // thinking parts joined.
        assert_eq!(m.thinking.as_deref(), Some("step1step2"));
        // images dropped from assistant.
        assert!(m.images.is_empty());
        // Two tool calls; first synthesized id -> None, second preserved.
        assert_eq!(m.tool_calls.len(), 2);
        assert_eq!(m.tool_calls[0].id, None);
        assert_eq!(m.tool_calls[0].function.name, "synth");
        // arguments is a JSON Value, not a string.
        assert_eq!(m.tool_calls[0].function.arguments, json!({"a": 1}));
        assert_eq!(m.tool_calls[1].id.as_deref(), Some("call_real"));
        assert_eq!(m.tool_calls[1].function.arguments, json!({"b": 2}));
    }

    #[test]
    fn assistant_without_thinking_has_none() {
        let assistant = Message::assistant_text("plain");
        let native = ir_to_native_request(req_with(vec![assistant]), false).unwrap();
        assert_eq!(native.messages[0].thinking, None);
    }

    #[test]
    fn tools_and_options_mapped() {
        let mut req = req_with(vec![Message::user_text("hi")]);
        req.tools = vec![Tool {
            name: "get_weather".into(),
            description: "weather".into(),
            input_schema: json!({"type": "object"}),
            cache_control: None,
        }];
        req.max_tokens = 999;
        req.temperature = Some(0.5);
        req.top_p = Some(0.9);
        req.top_k = Some(40);
        req.stop_sequences = vec!["STOP".into()];

        let native = ir_to_native_request(req, true).unwrap();
        assert!(native.stream);
        assert_eq!(native.tools.len(), 1);
        assert_eq!(native.tools[0].kind, "function");
        assert_eq!(native.tools[0].function.name, "get_weather");
        assert_eq!(native.tools[0].function.description, "weather");
        assert_eq!(
            native.tools[0].function.parameters,
            json!({"type": "object"})
        );

        assert_eq!(native.options.num_predict, Some(999));
        assert_eq!(native.options.temperature, Some(0.5));
        assert_eq!(native.options.top_p, Some(0.9));
        assert_eq!(native.options.top_k, Some(40));
        assert_eq!(native.options.stop, vec!["STOP".to_string()]);
    }

    // ---- native_response_to_ir ----

    #[test]
    fn response_thinking_first_then_text() {
        let msg = NativeMessage {
            role: "assistant".into(),
            content: "the answer".into(),
            thinking: Some("reasoning".into()),
            images: Vec::new(),
            tool_calls: Vec::new(),
        };
        let ir = native_response_to_ir(native_resp(msg, Some("stop"))).unwrap();
        assert_eq!(ir.message.content.len(), 2);
        match &ir.message.content[0] {
            ContentBlock::Thinking(t) => assert_eq!(t.thinking, "reasoning"),
            other => panic!("expected thinking first, got {other:?}"),
        }
        match &ir.message.content[1] {
            ContentBlock::Text(t) => assert_eq!(t.text, "the answer"),
            other => panic!("expected text second, got {other:?}"),
        }
        assert_eq!(ir.stop_reason, StopReason::EndTurn);
        assert_eq!(ir.id, "");
        assert_eq!(ir.model, "test-model");
    }

    #[test]
    fn response_empty_thinking_filtered() {
        let msg = NativeMessage {
            role: "assistant".into(),
            content: "answer".into(),
            thinking: Some(String::new()),
            images: Vec::new(),
            tool_calls: Vec::new(),
        };
        let ir = native_response_to_ir(native_resp(msg, Some("stop"))).unwrap();
        assert_eq!(ir.message.content.len(), 1);
        assert!(matches!(ir.message.content[0], ContentBlock::Text(_)));
    }

    #[test]
    fn response_empty_content_emits_no_text_block() {
        let mut msg = empty_native_msg();
        msg.thinking = Some("only thinking".into());
        let ir = native_response_to_ir(native_resp(msg, Some("stop"))).unwrap();
        assert_eq!(ir.message.content.len(), 1);
        assert!(matches!(ir.message.content[0], ContentBlock::Thinking(_)));
    }

    #[test]
    fn response_tool_calls_id_preserved_and_synthesized() {
        let msg = NativeMessage {
            role: "assistant".into(),
            content: String::new(),
            thinking: None,
            images: Vec::new(),
            tool_calls: vec![
                NativeToolCall {
                    id: Some("call_abc".into()),
                    function: NativeFunctionCall {
                        name: "first".into(),
                        arguments: json!({"x": 1}),
                    },
                },
                NativeToolCall {
                    id: None,
                    function: NativeFunctionCall {
                        name: "second".into(),
                        arguments: Value::Null,
                    },
                },
            ],
        };
        // done_reason "stop" but tool_calls present -> ToolUse forced.
        let ir = native_response_to_ir(native_resp(msg, Some("stop"))).unwrap();
        assert_eq!(ir.stop_reason, StopReason::ToolUse);
        assert_eq!(ir.message.content.len(), 2);
        match &ir.message.content[0] {
            ContentBlock::ToolUse(tu) => {
                assert_eq!(tu.id, "call_abc");
                assert_eq!(tu.name, "first");
                assert_eq!(tu.input, json!({"x": 1}));
            }
            other => panic!("expected tool use, got {other:?}"),
        }
        match &ir.message.content[1] {
            ContentBlock::ToolUse(tu) => {
                // None id synthesized as tool_{idx}; idx is 1 here.
                assert_eq!(tu.id, "tool_1");
                assert_eq!(tu.name, "second");
            }
            other => panic!("expected tool use, got {other:?}"),
        }
    }

    #[test]
    fn response_usage_and_done_reason() {
        let mut resp = native_resp(empty_native_msg(), Some("length"));
        resp.prompt_eval_count = 12;
        resp.eval_count = 34;
        let ir = native_response_to_ir(resp).unwrap();
        assert_eq!(ir.usage.input_tokens, 12);
        assert_eq!(ir.usage.output_tokens, 34);
        assert_eq!(ir.usage.cache_creation_input_tokens, None);
        assert_eq!(ir.usage.cache_read_input_tokens, None);
        // No tool calls, done_reason "length" -> MaxTokens.
        assert_eq!(ir.stop_reason, StopReason::MaxTokens);
        // No content at all.
        assert!(ir.message.content.is_empty());
        assert_eq!(ir.message.role, Role::Assistant);
    }

    // ---- map_done_reason ----

    #[test]
    fn map_done_reason_table() {
        assert_eq!(map_done_reason(Some("length")), StopReason::MaxTokens);
        assert_eq!(map_done_reason(Some("tool_calls")), StopReason::ToolUse);
        assert_eq!(map_done_reason(Some("stop")), StopReason::EndTurn);
        assert_eq!(map_done_reason(None), StopReason::EndTurn);
        assert_eq!(map_done_reason(Some("other")), StopReason::EndTurn);
    }
}
