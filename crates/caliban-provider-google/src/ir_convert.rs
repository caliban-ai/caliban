//! IR ↔ Gemini native conversions for request and response.

use caliban_provider::{
    ContentBlock, Error, ImageSource as IrImageSource, Message, Result, Role, StopReason,
    TextBlock as IrTextBlock, Tool as IrTool, ToolChoice as IrToolChoice,
    ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
};

use crate::schema::request::{
    NativeContent, NativeFunctionCall, NativeFunctionCallingConfig, NativeFunctionDeclaration,
    NativeFunctionResponse, NativeGenerationConfig, NativeInlineData, NativePart, NativeRequest,
    NativeSystemInstruction, NativeToolConfig, NativeToolList,
};
use crate::schema::response::{NativeFinishReason, NativeResponse};

/// Convert a caliban IR `CompletionRequest` to a Gemini `NativeRequest`.
///
/// Leading `Role::System` messages are collected into `systemInstruction`.
/// `Role::User` → `"user"`, `Role::Assistant` → `"model"`.
///
/// # Errors
///
/// Returns `Err` if an `Image::Url` block is encountered (not supported for AI Studio),
/// or if a `ToolResult` block's content cannot be serialized to JSON.
///
/// # Panics
///
/// This function cannot panic in practice; the `expect` is guarded by a preceding `peek`.
#[allow(clippy::too_many_lines)]
pub fn ir_to_native_request(req: caliban_provider::CompletionRequest) -> Result<NativeRequest> {
    let mut messages_iter = req.messages.into_iter().peekable();

    // Collect leading System messages into systemInstruction.
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

    let system_instruction = if system_texts.is_empty() {
        None
    } else {
        Some(NativeSystemInstruction {
            parts: vec![NativePart::Text(system_texts.join("\n\n"))],
        })
    };

    // We need to build a lookup of tool_use_id -> function name for ToolResult correlation.
    // We pre-scan all messages to build this map.
    let mut id_to_name: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    let remaining: Vec<_> = messages_iter.collect();
    for msg in &remaining {
        if msg.role == Role::Assistant {
            for cb in &msg.content {
                if let ContentBlock::ToolUse(tu) = cb {
                    id_to_name.insert(tu.id.clone(), tu.name.clone());
                }
            }
        }
    }

    let mut contents: Vec<NativeContent> = Vec::new();

    for msg in remaining {
        match msg.role {
            Role::System => {
                // Non-leading system messages: append text to a "user" message as a fallback.
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
                if !text.is_empty() {
                    contents.push(NativeContent {
                        role: "user".into(),
                        parts: vec![NativePart::Text(text)],
                    });
                }
            }
            Role::User => {
                let mut parts: Vec<NativePart> = Vec::new();
                for cb in msg.content {
                    match cb {
                        ContentBlock::Text(tb) => {
                            parts.push(NativePart::Text(tb.text));
                        }
                        ContentBlock::Image(img) => match img.source {
                            IrImageSource::Base64 { media_type, data } => {
                                parts.push(NativePart::InlineData(NativeInlineData {
                                    mime_type: media_type,
                                    data,
                                }));
                            }
                            IrImageSource::Url { .. } => {
                                return Err(Error::InvalidRequest(
                                    "Google AI Studio requires base64 images; got URL".into(),
                                ));
                            }
                        },
                        ContentBlock::ToolResult(tr) => {
                            // Correlate by tool_use_id → function name.
                            let fn_name = id_to_name
                                .get(&tr.tool_use_id)
                                .cloned()
                                .unwrap_or_else(|| tr.tool_use_id.clone());

                            // Build the response value from text content.
                            let response_text = tr
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
                            let response_value =
                                serde_json::Value::Object(serde_json::Map::from_iter([(
                                    "output".to_string(),
                                    serde_json::Value::String(response_text),
                                )]));

                            parts.push(NativePart::FunctionResponse(NativeFunctionResponse {
                                name: fn_name,
                                response: response_value,
                            }));
                        }
                        // Thinking blocks and unexpected ToolUse in User messages are dropped.
                        ContentBlock::Thinking(_) | ContentBlock::ToolUse(_) => {}
                    }
                }
                if !parts.is_empty() {
                    contents.push(NativeContent {
                        role: "user".into(),
                        parts,
                    });
                }
            }
            Role::Assistant => {
                let mut parts: Vec<NativePart> = Vec::new();
                for cb in msg.content {
                    match cb {
                        ContentBlock::Text(tb) => {
                            parts.push(NativePart::Text(tb.text));
                        }
                        ContentBlock::ToolUse(tu) => {
                            parts.push(NativePart::FunctionCall(NativeFunctionCall {
                                name: tu.name,
                                args: tu.input,
                            }));
                        }
                        // Thinking, Image, cache control dropped.
                        ContentBlock::Thinking(_)
                        | ContentBlock::Image(_)
                        | ContentBlock::ToolResult(_) => {}
                    }
                }
                if !parts.is_empty() {
                    contents.push(NativeContent {
                        role: "model".into(),
                        parts,
                    });
                }
            }
        }
    }

    // Tool config
    let tool_config = match req.tool_choice {
        IrToolChoice::Auto => Some(NativeToolConfig {
            function_calling_config: NativeFunctionCallingConfig {
                mode: "AUTO".into(),
                allowed_function_names: Vec::new(),
            },
        }),
        IrToolChoice::Any => Some(NativeToolConfig {
            function_calling_config: NativeFunctionCallingConfig {
                mode: "ANY".into(),
                allowed_function_names: Vec::new(),
            },
        }),
        IrToolChoice::Specific { name } => Some(NativeToolConfig {
            function_calling_config: NativeFunctionCallingConfig {
                mode: "ANY".into(),
                allowed_function_names: vec![name],
            },
        }),
        IrToolChoice::None => Some(NativeToolConfig {
            function_calling_config: NativeFunctionCallingConfig {
                mode: "NONE".into(),
                allowed_function_names: Vec::new(),
            },
        }),
    };

    // Tool declarations
    let tools: Vec<NativeToolList> = if req.tools.is_empty() {
        Vec::new()
    } else {
        let declarations: Vec<NativeFunctionDeclaration> = req
            .tools
            .into_iter()
            .map(|t: IrTool| NativeFunctionDeclaration {
                name: t.name,
                description: t.description,
                parameters: t.input_schema,
            })
            .collect();
        vec![NativeToolList {
            function_declarations: declarations,
        }]
    };

    // Generation config — only emit if there's something to say.
    let has_gen_config = req.max_tokens > 0
        || req.temperature.is_some()
        || req.top_p.is_some()
        || req.top_k.is_some()
        || !req.stop_sequences.is_empty();

    let generation_config = if has_gen_config {
        Some(NativeGenerationConfig {
            max_output_tokens: Some(req.max_tokens),
            temperature: req.temperature,
            top_p: req.top_p,
            top_k: req.top_k,
            stop_sequences: req.stop_sequences,
        })
    } else {
        None
    };

    Ok(NativeRequest {
        contents,
        system_instruction,
        tools,
        tool_config,
        generation_config,
    })
}

/// Convert a Gemini `NativeResponse` to a caliban `CompletionResponse`.
///
/// # Errors
///
/// Returns `Err` if the response has no candidates.
pub fn native_response_to_ir(r: NativeResponse) -> Result<caliban_provider::CompletionResponse> {
    let candidate = r
        .candidates
        .into_iter()
        .next()
        .ok_or_else(|| Error::InvalidRequest("Gemini response has no candidates".into()))?;

    let finish_reason = candidate.finish_reason;
    let model_version = r.model_version.clone();

    let mut content_blocks: Vec<ContentBlock> = Vec::new();
    let mut tool_idx: u32 = 0;

    for part in candidate.content.parts {
        match part {
            NativePart::Text(s) => {
                if !s.is_empty() {
                    content_blocks.push(ContentBlock::Text(IrTextBlock {
                        text: s,
                        cache_control: None,
                    }));
                }
            }
            NativePart::FunctionCall(fc) => {
                let id = format!("toolu_{tool_idx}");
                tool_idx += 1;
                content_blocks.push(ContentBlock::ToolUse(IrToolUseBlock {
                    id,
                    name: fc.name,
                    input: fc.args,
                }));
            }
            // InlineData and FunctionResponse in responses are ignored.
            NativePart::InlineData(_) | NativePart::FunctionResponse(_) => {}
        }
    }

    let stop_reason = map_finish_reason(finish_reason);

    Ok(caliban_provider::CompletionResponse {
        id: model_version,
        model: r.model_version,
        message: Message {
            role: Role::Assistant,
            content: content_blocks,
        },
        stop_reason,
        stop_sequence: None,
        usage: IrUsage {
            input_tokens: r.usage_metadata.prompt_token_count,
            output_tokens: r.usage_metadata.candidates_token_count,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    })
}

/// Map a Gemini finish reason to an IR `StopReason`.
pub(crate) fn map_finish_reason(reason: Option<NativeFinishReason>) -> StopReason {
    match reason {
        Some(NativeFinishReason::MaxTokens) => StopReason::MaxTokens,
        Some(NativeFinishReason::Safety | NativeFinishReason::Recitation) => {
            StopReason::ContentFilter
        }
        Some(NativeFinishReason::ToolUse) => StopReason::ToolUse,
        Some(
            NativeFinishReason::Stop
            | NativeFinishReason::Other
            | NativeFinishReason::FinishReasonUnspecified,
        )
        | None => StopReason::EndTurn,
    }
}
