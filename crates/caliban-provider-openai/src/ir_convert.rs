//! IR ↔ `OpenAI` native conversions for request and response.

use caliban_provider::{
    ContentBlock, Error, ImageSource as IrImageSource, Message, Result, Role, StopReason,
    TextBlock as IrTextBlock, Tool as IrTool, ToolChoice as IrToolChoice,
    ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
};

use crate::models::uses_completion_tokens;
use crate::schema::request::{
    NativeContent, NativeContentPart, NativeFunctionCall, NativeImageUrl, NativeMessage,
    NativeRequest, NativeStreamOptions, NativeTool, NativeToolCall, NativeToolChoice,
    NativeToolFunction, NativeToolFunctionName,
};
use crate::schema::response::{NativeFinishReason, NativeResponse};

/// Convert a caliban IR `CompletionRequest` to an `OpenAI` `NativeRequest`.
///
/// `system_role` controls the `"role"` string used for system messages.  Pass
/// `"system"` for standard models and `"developer"` for o1-series models.
///
/// # Errors
///
/// Returns `Err` if a `ToolUseBlock`'s `input` value cannot be serialized to a JSON string.
///
/// # Panics
///
/// This function cannot panic in practice; the `expect` is guarded by a preceding `peek`.
#[allow(clippy::too_many_lines)]
pub fn ir_to_native_request(
    req: caliban_provider::CompletionRequest,
    stream: bool,
    system_role: &str,
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

    // Prepend system message if any leading system content existed.
    if !system_texts.is_empty() {
        native_messages.push(NativeMessage {
            role: system_role.into(),
            content: Some(NativeContent::Text(system_texts.join("\n\n"))),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        });
    }

    // Convert remaining User/Assistant messages.
    for msg in messages_iter {
        match msg.role {
            Role::System => {
                // Validated-out by CompletionRequest::validate(), but handle gracefully.
                // Concatenate and append as another system message.
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
                    role: system_role.into(),
                    content: Some(NativeContent::Text(text)),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    name: None,
                });
            }
            Role::User => {
                // Check for ToolResult blocks — each becomes a separate tool message.
                let mut has_non_tool_result = false;
                let mut tool_result_msgs: Vec<NativeMessage> = Vec::new();
                let mut user_parts: Vec<NativeContentPart> = Vec::new();

                for cb in msg.content {
                    match cb {
                        ContentBlock::ToolResult(tr) => {
                            // Concatenate text content; drop images silently.
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
                                content: Some(NativeContent::Text(content_text)),
                                tool_calls: Vec::new(),
                                tool_call_id: Some(tr.tool_use_id),
                                name: None,
                            });
                        }
                        ContentBlock::Text(tb) => {
                            has_non_tool_result = true;
                            user_parts.push(NativeContentPart::Text { text: tb.text });
                        }
                        ContentBlock::Image(img) => {
                            has_non_tool_result = true;
                            let url = match img.source {
                                IrImageSource::Base64 { media_type, data } => {
                                    format!("data:{media_type};base64,{data}")
                                }
                                IrImageSource::Url { url } => url,
                                IrImageSource::BlobRef { media_type, .. } => {
                                    // Should have been resolved by the
                                    // session layer; emit an empty data URL
                                    // so the wire shape stays well-formed
                                    // and the provider can reject it.
                                    format!("data:{media_type};base64,")
                                }
                            };
                            user_parts.push(NativeContentPart::ImageUrl {
                                image_url: NativeImageUrl { url },
                            });
                        }
                        // Thinking blocks and unexpected ToolUse in User messages are dropped.
                        ContentBlock::Thinking(_) | ContentBlock::ToolUse(_) => {}
                    }
                }

                // If there were non-tool-result content items, emit a user message first.
                if has_non_tool_result {
                    let content = if user_parts.len() == 1 {
                        if let NativeContentPart::Text { text } = &user_parts[0] {
                            NativeContent::Text(text.clone())
                        } else {
                            NativeContent::Parts(user_parts)
                        }
                    } else {
                        NativeContent::Parts(user_parts)
                    };
                    native_messages.push(NativeMessage {
                        role: "user".into(),
                        content: Some(content),
                        tool_calls: Vec::new(),
                        tool_call_id: None,
                        name: None,
                    });
                }

                // Append tool result messages after the user message.
                native_messages.extend(tool_result_msgs);
            }
            Role::Assistant => {
                // Separate text blocks from tool-use blocks.
                let mut text_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<NativeToolCall> = Vec::new();

                for cb in msg.content {
                    match cb {
                        ContentBlock::Text(tb) => {
                            text_parts.push(tb.text);
                        }
                        ContentBlock::ToolUse(tu) => {
                            let arguments = serde_json::to_string(&tu.input).map_err(|e| {
                                Error::InvalidRequest(format!(
                                    "failed to serialize tool_use input: {e}"
                                ))
                            })?;
                            tool_calls.push(NativeToolCall {
                                id: tu.id,
                                kind: "function".into(),
                                function: NativeFunctionCall {
                                    name: tu.name,
                                    arguments,
                                },
                            });
                        }
                        // Thinking, Image, and unexpected ToolResult in assistant messages are dropped.
                        ContentBlock::Thinking(_)
                        | ContentBlock::Image(_)
                        | ContentBlock::ToolResult(_) => {}
                    }
                }

                let content = if text_parts.is_empty() {
                    None
                } else {
                    Some(NativeContent::Text(text_parts.join("")))
                };

                native_messages.push(NativeMessage {
                    role: "assistant".into(),
                    content,
                    tool_calls,
                    tool_call_id: None,
                    name: None,
                });
            }
        }
    }

    let tool_choice = match req.tool_choice {
        IrToolChoice::Auto => Some(NativeToolChoice::Auto("auto".into())),
        IrToolChoice::Any => Some(NativeToolChoice::Auto("required".into())),
        IrToolChoice::Specific { name } => Some(NativeToolChoice::Specific {
            kind: "function".into(),
            function: NativeToolFunctionName { name },
        }),
        IrToolChoice::None => None,
    };

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

    // GPT-5 / o-series reject `max_tokens`; route the cap to
    // `max_completion_tokens` for those families. Lmstudio probe Finding 6.
    let (max_tokens, max_completion_tokens) = if uses_completion_tokens(&req.model) {
        (None, Some(req.max_tokens))
    } else {
        (Some(req.max_tokens), None)
    };

    // Per OpenAI streaming spec, the terminal `usage` chunk is only emitted
    // when `stream_options.include_usage = true`. Lmstudio probe Finding 1.
    let stream_options = if stream {
        Some(NativeStreamOptions {
            include_usage: true,
        })
    } else {
        None
    };

    Ok(NativeRequest {
        model: req.model,
        messages: native_messages,
        tools,
        tool_choice,
        max_tokens,
        max_completion_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        stop: req.stop_sequences,
        user: req.metadata.user_id,
        stream,
        stream_options,
    })
}

/// Convert an `OpenAI` `NativeResponse` to a caliban `CompletionResponse`.
///
/// # Errors
///
/// Returns `Err` if the response has no choices, or if `tool_calls[i].function.arguments`
/// is not valid JSON.
pub fn native_response_to_ir(r: NativeResponse) -> Result<caliban_provider::CompletionResponse> {
    let choice = r
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| Error::InvalidRequest("OpenAI response has no choices".into()))?;

    let msg = choice.message;
    let finish_reason = choice.finish_reason;

    let mut content_blocks: Vec<caliban_provider::ContentBlock> = Vec::new();

    // Handle refusal first — map to a Text block and use Refusal stop reason.
    let stop_reason = if let Some(ref refusal_text) = msg.refusal {
        content_blocks.push(caliban_provider::ContentBlock::Text(IrTextBlock {
            text: refusal_text.clone(),
            cache_control: None,
        }));
        StopReason::Refusal
    } else {
        // Map finish_reason to StopReason.
        match finish_reason {
            NativeFinishReason::Stop => StopReason::EndTurn,
            NativeFinishReason::Length => StopReason::MaxTokens,
            NativeFinishReason::ToolCalls | NativeFinishReason::FunctionCall => StopReason::ToolUse,
            NativeFinishReason::ContentFilter => StopReason::ContentFilter,
        }
    };

    // If there is text content (and no refusal was already added), add it.
    if msg.refusal.is_none() {
        if let Some(text) = msg.content
            && !text.is_empty()
        {
            content_blocks.push(caliban_provider::ContentBlock::Text(IrTextBlock {
                text,
                cache_control: None,
            }));
        }

        // Convert tool calls to ToolUse blocks.
        for tc in msg.tool_calls {
            let input: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).map_err(|e| {
                    Error::InvalidRequest(format!(
                        "tool_call '{}' arguments is not valid JSON: {e}",
                        tc.function.name
                    ))
                })?;
            content_blocks.push(caliban_provider::ContentBlock::ToolUse(IrToolUseBlock {
                id: tc.id,
                name: tc.function.name,
                input,
            }));
        }
    }

    let cache_read = r
        .usage
        .prompt_tokens_details
        .filter(|d| d.cached_tokens > 0)
        .map(|d| d.cached_tokens);

    Ok(caliban_provider::CompletionResponse {
        id: r.id,
        model: r.model,
        message: Message {
            role: Role::Assistant,
            content: content_blocks,
        },
        stop_reason,
        stop_sequence: None,
        usage: IrUsage {
            input_tokens: r.usage.prompt_tokens,
            output_tokens: r.usage.completion_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: cache_read,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{CompletionRequest, RequestMetadata};

    fn minimal_request(model: &str) -> CompletionRequest {
        CompletionRequest {
            model: model.into(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text(IrTextBlock {
                    text: "hi".into(),
                    cache_control: None,
                })],
            }],
            tools: vec![],
            tool_choice: IrToolChoice::default(),
            max_tokens: 256,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            metadata: RequestMetadata::default(),
        }
    }

    #[test]
    fn streaming_request_sets_include_usage_true() {
        let native =
            ir_to_native_request(minimal_request("gpt-4o"), true, "system").expect("ir_to_native");
        assert!(native.stream);
        assert_eq!(
            native.stream_options,
            Some(NativeStreamOptions {
                include_usage: true,
            })
        );
    }

    #[test]
    fn non_streaming_request_omits_stream_options() {
        let native =
            ir_to_native_request(minimal_request("gpt-4o"), false, "system").expect("ir_to_native");
        assert!(!native.stream);
        assert_eq!(native.stream_options, None);
    }

    #[test]
    fn gpt5_routes_to_max_completion_tokens() {
        let native =
            ir_to_native_request(minimal_request("gpt-5"), false, "system").expect("ir_to_native");
        assert_eq!(native.max_tokens, None);
        assert_eq!(native.max_completion_tokens, Some(256));
    }

    #[test]
    fn gpt4o_uses_legacy_max_tokens() {
        let native =
            ir_to_native_request(minimal_request("gpt-4o"), false, "system").expect("ir_to_native");
        assert_eq!(native.max_tokens, Some(256));
        assert_eq!(native.max_completion_tokens, None);
    }

    #[test]
    fn o_series_models_route_to_max_completion_tokens() {
        for model in ["o1-mini", "o3-mini", "o4-mini"] {
            let native = ir_to_native_request(minimal_request(model), false, "system")
                .expect("ir_to_native");
            assert_eq!(
                native.max_tokens, None,
                "{model} should not send max_tokens"
            );
            assert_eq!(
                native.max_completion_tokens,
                Some(256),
                "{model} should send max_completion_tokens"
            );
        }
    }

    #[test]
    fn case_insensitive_model_family_match() {
        for model in ["GPT-5", "O1", "O3-MINI"] {
            let native = ir_to_native_request(minimal_request(model), false, "system")
                .expect("ir_to_native");
            assert_eq!(
                native.max_tokens, None,
                "{model} should not send max_tokens"
            );
            assert_eq!(
                native.max_completion_tokens,
                Some(256),
                "{model} should send max_completion_tokens"
            );
        }
    }
}
