//! IR ↔ Ollama native conversions for request and response.

use caliban_provider::{
    ContentBlock, Error, ImageSource as IrImageSource, Message, Result, Role, StopReason,
    TextBlock as IrTextBlock, Tool as IrTool, ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
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
                        images,
                        tool_calls: Vec::new(),
                    });
                }

                // Append tool result messages after the user message.
                native_messages.extend(tool_result_msgs);
            }
            Role::Assistant => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<NativeToolCall> = Vec::new();

                for cb in msg.content {
                    match cb {
                        ContentBlock::Text(tb) => {
                            text_parts.push(tb.text);
                        }
                        ContentBlock::ToolUse(tu) => {
                            // Ollama arguments is a JSON Value object, NOT a string.
                            tool_calls.push(NativeToolCall {
                                function: NativeFunctionCall {
                                    name: tu.name,
                                    arguments: tu.input,
                                },
                            });
                        }
                        // Thinking, Image, and unexpected ToolResult in assistant messages are dropped.
                        ContentBlock::Thinking(_)
                        | ContentBlock::Image(_)
                        | ContentBlock::ToolResult(_) => {}
                    }
                }

                native_messages.push(NativeMessage {
                    role: "assistant".into(),
                    content: text_parts.join(""),
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

    let mut content_blocks: Vec<caliban_provider::ContentBlock> = Vec::new();

    // Map text content.
    if !msg.content.is_empty() {
        content_blocks.push(caliban_provider::ContentBlock::Text(IrTextBlock {
            text: msg.content,
            cache_control: None,
        }));
    }

    // Map tool calls.
    for (idx, tc) in msg.tool_calls.into_iter().enumerate() {
        content_blocks.push(caliban_provider::ContentBlock::ToolUse(IrToolUseBlock {
            id: format!("tool_{idx}"),
            name: tc.function.name,
            input: tc.function.arguments,
        }));
    }

    let stop_reason = map_done_reason(r.done_reason.as_deref());

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
pub(crate) fn map_done_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("length") => StopReason::MaxTokens,
        Some("tool_calls") => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    }
}
