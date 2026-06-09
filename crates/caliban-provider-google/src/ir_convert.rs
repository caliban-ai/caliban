//! IR ↔ Gemini native conversions for request and response.

use caliban_provider::{
    ContentBlock, Error, ImageSource as IrImageSource, Message, Result, Role, StopReason,
    TextBlock as IrTextBlock, Tool as IrTool, ToolChoice as IrToolChoice,
    ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
};

use crate::schema::request::{
    NativeContent, NativeFileData, NativeFunctionCall, NativeFunctionCallingConfig,
    NativeFunctionDeclaration, NativeFunctionResponse, NativeGenerationConfig, NativeInlineData,
    NativePart, NativeRequest, NativeSystemInstruction, NativeToolConfig, NativeToolList,
};
use crate::schema::response::{NativeFinishReason, NativeResponse};

/// Convert a caliban IR `CompletionRequest` to a Gemini `NativeRequest`.
///
/// Leading `Role::System` messages are collected into `systemInstruction`.
/// `Role::User` → `"user"`, `Role::Assistant` → `"model"`.
///
/// Set `allow_url_images` to `true` when targeting Vertex AI (which supports `fileData`
/// URI parts). Set it to `false` for AI Studio, which requires base64 inline data and
/// will return an error if a URL image is encountered.
///
/// # Errors
///
/// Returns `Err` if `allow_url_images` is `false` and an `Image::Url` block is
/// encountered, or if a `ToolResult` block's content cannot be serialized to JSON.
///
/// # Panics
///
/// This function cannot panic in practice; the `expect` is guarded by a preceding `peek`.
#[allow(clippy::too_many_lines)]
pub fn ir_to_native_request(
    req: caliban_provider::CompletionRequest,
    allow_url_images: bool,
) -> Result<NativeRequest> {
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
                            IrImageSource::Url { url } => {
                                if allow_url_images {
                                    // Vertex AI supports fileData URI parts.
                                    // We can't always know the MIME type from the URL
                                    // alone; use a generic fallback — callers that know
                                    // the MIME type should use Base64 or supply it at
                                    // a higher layer.
                                    let mime = infer_mime_from_url(&url);
                                    parts.push(NativePart::FileData(NativeFileData {
                                        mime_type: mime,
                                        file_uri: url,
                                    }));
                                } else {
                                    return Err(Error::InvalidRequest(
                                        "Google AI Studio requires base64 images; got URL".into(),
                                    ));
                                }
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
            // InlineData, FileData, and FunctionResponse in responses are ignored.
            NativePart::InlineData(_)
            | NativePart::FileData(_)
            | NativePart::FunctionResponse(_) => {}
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
            // Gemini's context caching uses a separate `cachedContents` API
            // resource rather than per-block markers. Not yet implemented;
            // revisit when adding a Gemini caching slice.
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    })
}

/// Infer a MIME type from a URL's file extension.
///
/// Falls back to `"application/octet-stream"` for unknown extensions.
fn infer_mime_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png".to_string(),
        Some("jpg" | "jpeg") => "image/jpeg".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("webp") => "image/webp".to_string(),
        Some("pdf") => "application/pdf".to_string(),
        Some("mp4") => "video/mp4".to_string(),
        Some("mp3") => "audio/mpeg".to_string(),
        Some("wav") => "audio/wav".to_string(),
        _ => "application/octet-stream".to_string(),
    }
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

#[cfg(test)]
#[allow(clippy::too_many_lines)]
mod tests {
    use super::*;
    use crate::schema::response::{NativeCandidate, NativeUsageMetadata};
    use caliban_provider::{
        CompletionRequest, ContentBlock, ImageBlock, ImageSource, Message, RequestMetadata, Role,
        TextBlock, Tool, ToolChoice, ToolResultBlock, ToolUseBlock,
    };

    /// Build a minimal `CompletionRequest` from a message list, bypassing
    /// `validate()` so we can construct otherwise-invalid orderings.
    fn req_with(messages: Vec<Message>) -> CompletionRequest {
        CompletionRequest {
            model: "gemini-test".into(),
            messages,
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            max_tokens: 0,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            effort: None,
            metadata: RequestMetadata::default(),
        }
    }

    fn text_block(s: &str) -> ContentBlock {
        ContentBlock::Text(TextBlock {
            text: s.into(),
            cache_control: None,
        })
    }

    #[test]
    fn leading_system_messages_joined_into_instruction() {
        let req = req_with(vec![
            Message::system_text("first"),
            Message::system_text("second"),
            Message::user_text("hello"),
        ]);
        let native = ir_to_native_request(req, false).unwrap();
        let si = native
            .system_instruction
            .expect("system instruction present");
        assert_eq!(si.parts, vec![NativePart::Text("first\n\nsecond".into())]);
    }

    #[test]
    fn no_system_message_yields_none_instruction() {
        let req = req_with(vec![Message::user_text("hi")]);
        let native = ir_to_native_request(req, false).unwrap();
        assert!(native.system_instruction.is_none());
    }

    #[test]
    fn assistant_tool_use_becomes_function_call() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse(ToolUseBlock {
                id: "id1".into(),
                name: "get_weather".into(),
                input: serde_json::json!({"city": "NYC"}),
            })],
        }]);
        let native = ir_to_native_request(req, false).unwrap();
        assert_eq!(native.contents.len(), 1);
        assert_eq!(native.contents[0].role, "model");
        assert_eq!(
            native.contents[0].parts,
            vec![NativePart::FunctionCall(NativeFunctionCall {
                name: "get_weather".into(),
                args: serde_json::json!({"city": "NYC"}),
            })]
        );
    }

    #[test]
    fn user_text_and_base64_image_become_text_and_inline_data() {
        let req = req_with(vec![Message {
            role: Role::User,
            content: vec![
                text_block("look"),
                ContentBlock::Image(ImageBlock {
                    source: ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
            ],
        }]);
        let native = ir_to_native_request(req, false).unwrap();
        assert_eq!(native.contents[0].role, "user");
        assert_eq!(
            native.contents[0].parts,
            vec![
                NativePart::Text("look".into()),
                NativePart::InlineData(NativeInlineData {
                    mime_type: "image/png".into(),
                    data: "AAAA".into(),
                }),
            ]
        );
    }

    #[test]
    fn user_url_image_with_allow_becomes_file_data() {
        let req = req_with(vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source: ImageSource::Url {
                    url: "https://example.com/pic.png".into(),
                },
                cache_control: None,
                sha256: None,
                dims: None,
            })],
        }]);
        let native = ir_to_native_request(req, true).unwrap();
        assert_eq!(
            native.contents[0].parts,
            vec![NativePart::FileData(NativeFileData {
                mime_type: "image/png".into(),
                file_uri: "https://example.com/pic.png".into(),
            })]
        );
    }

    #[test]
    fn user_url_image_without_allow_errors() {
        let req = req_with(vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source: ImageSource::Url {
                    url: "https://example.com/pic.png".into(),
                },
                cache_control: None,
                sha256: None,
                dims: None,
            })],
        }]);
        let err = ir_to_native_request(req, false).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
    }

    #[test]
    fn user_blobref_image_errors() {
        let req = req_with(vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source: ImageSource::BlobRef {
                    sha256: "abc".into(),
                    media_type: "image/png".into(),
                },
                cache_control: None,
                sha256: None,
                dims: None,
            })],
        }]);
        let err = ir_to_native_request(req, true).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
    }

    #[test]
    fn tool_result_resolves_name_from_prior_tool_use() {
        let req = req_with(vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse(ToolUseBlock {
                    id: "call_1".into(),
                    name: "search".into(),
                    input: serde_json::json!({}),
                })],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult(ToolResultBlock {
                    tool_use_id: "call_1".into(),
                    content: vec![text_block("result text")],
                    is_error: false,
                })],
            },
        ]);
        let native = ir_to_native_request(req, false).unwrap();
        // contents[0] = model function call, contents[1] = user function response
        assert_eq!(native.contents.len(), 2);
        assert_eq!(
            native.contents[1].parts,
            vec![NativePart::FunctionResponse(NativeFunctionResponse {
                name: "search".into(),
                response: serde_json::json!({"output": "result text"}),
            })]
        );
    }

    #[test]
    fn tool_result_unknown_id_falls_back_to_id_as_name() {
        let req = req_with(vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: "orphan_id".into(),
                content: vec![text_block("data")],
                is_error: false,
            })],
        }]);
        let native = ir_to_native_request(req, false).unwrap();
        assert_eq!(
            native.contents[0].parts,
            vec![NativePart::FunctionResponse(NativeFunctionResponse {
                name: "orphan_id".into(),
                response: serde_json::json!({"output": "data"}),
            })]
        );
    }

    #[test]
    fn non_leading_system_message_becomes_user_text() {
        // User first so the System message is non-leading; bypass validate via literal.
        let req = req_with(vec![
            Message::user_text("hi"),
            Message::system_text("mid-convo system note"),
        ]);
        let native = ir_to_native_request(req, false).unwrap();
        // No leading system => no instruction.
        assert!(native.system_instruction.is_none());
        // Two user contents: original user msg, then the demoted system text.
        assert_eq!(native.contents.len(), 2);
        assert_eq!(native.contents[1].role, "user");
        assert_eq!(
            native.contents[1].parts,
            vec![NativePart::Text("mid-convo system note".into())]
        );
    }

    #[test]
    fn tool_choice_auto() {
        let mut req = req_with(vec![Message::user_text("x")]);
        req.tool_choice = ToolChoice::Auto;
        let native = ir_to_native_request(req, false).unwrap();
        let cfg = native.tool_config.unwrap().function_calling_config;
        assert_eq!(cfg.mode, "AUTO");
        assert!(cfg.allowed_function_names.is_empty());
    }

    #[test]
    fn tool_choice_any() {
        let mut req = req_with(vec![Message::user_text("x")]);
        req.tool_choice = ToolChoice::Any;
        let native = ir_to_native_request(req, false).unwrap();
        let cfg = native.tool_config.unwrap().function_calling_config;
        assert_eq!(cfg.mode, "ANY");
        assert!(cfg.allowed_function_names.is_empty());
    }

    #[test]
    fn tool_choice_specific() {
        let mut req = req_with(vec![Message::user_text("x")]);
        req.tool_choice = ToolChoice::Specific {
            name: "my_tool".into(),
        };
        let native = ir_to_native_request(req, false).unwrap();
        let cfg = native.tool_config.unwrap().function_calling_config;
        assert_eq!(cfg.mode, "ANY");
        assert_eq!(cfg.allowed_function_names, vec!["my_tool".to_string()]);
    }

    #[test]
    fn tool_choice_none() {
        let mut req = req_with(vec![Message::user_text("x")]);
        req.tool_choice = ToolChoice::None;
        let native = ir_to_native_request(req, false).unwrap();
        let cfg = native.tool_config.unwrap().function_calling_config;
        assert_eq!(cfg.mode, "NONE");
        assert!(cfg.allowed_function_names.is_empty());
    }

    #[test]
    fn tool_declarations_mapped() {
        let mut req = req_with(vec![Message::user_text("x")]);
        req.tools = vec![Tool {
            name: "lookup".into(),
            description: "looks things up".into(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        }];
        let native = ir_to_native_request(req, false).unwrap();
        assert_eq!(native.tools.len(), 1);
        let decl = &native.tools[0].function_declarations[0];
        assert_eq!(decl.name, "lookup");
        assert_eq!(decl.description, "looks things up");
        assert_eq!(decl.parameters, serde_json::json!({"type": "object"}));
    }

    #[test]
    fn generation_config_emitted_when_params_set() {
        let mut req = req_with(vec![Message::user_text("x")]);
        req.max_tokens = 256;
        req.temperature = Some(0.7);
        let native = ir_to_native_request(req, false).unwrap();
        let gc = native.generation_config.expect("generation config present");
        assert_eq!(gc.max_output_tokens, Some(256));
        let temp = gc.temperature.unwrap();
        assert!((temp - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn generation_config_none_when_all_unset() {
        // req_with sets max_tokens = 0 and all sampling params to None.
        let req = req_with(vec![Message::user_text("x")]);
        let native = ir_to_native_request(req, false).unwrap();
        assert!(native.generation_config.is_none());
    }

    // ---- native_response_to_ir ----

    fn response_with(
        parts: Vec<NativePart>,
        finish_reason: Option<NativeFinishReason>,
    ) -> NativeResponse {
        NativeResponse {
            candidates: vec![NativeCandidate {
                content: NativeContent {
                    role: "model".into(),
                    parts,
                },
                finish_reason,
                index: 0,
            }],
            usage_metadata: NativeUsageMetadata {
                prompt_token_count: 11,
                candidates_token_count: 22,
                total_token_count: 33,
            },
            model_version: "gemini-1.5-pro".into(),
        }
    }

    #[test]
    fn response_no_candidates_errors() {
        let r = NativeResponse {
            candidates: vec![],
            usage_metadata: NativeUsageMetadata::default(),
            model_version: "m".into(),
        };
        let err = native_response_to_ir(r).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
    }

    #[test]
    fn response_text_part_nonempty_kept_empty_dropped() {
        let r = response_with(
            vec![
                NativePart::Text(String::new()),
                NativePart::Text("hello".into()),
            ],
            Some(NativeFinishReason::Stop),
        );
        let ir = native_response_to_ir(r).unwrap();
        assert_eq!(ir.message.content.len(), 1);
        match &ir.message.content[0] {
            ContentBlock::Text(tb) => assert_eq!(tb.text, "hello"),
            other => panic!("expected text block, got {other:?}"),
        }
    }

    #[test]
    fn response_function_calls_get_sequential_ids() {
        let r = response_with(
            vec![
                NativePart::FunctionCall(NativeFunctionCall {
                    name: "a".into(),
                    args: serde_json::json!({}),
                }),
                NativePart::FunctionCall(NativeFunctionCall {
                    name: "b".into(),
                    args: serde_json::json!({"k": 1}),
                }),
            ],
            Some(NativeFinishReason::ToolUse),
        );
        let ir = native_response_to_ir(r).unwrap();
        assert_eq!(ir.message.content.len(), 2);
        match (&ir.message.content[0], &ir.message.content[1]) {
            (ContentBlock::ToolUse(a), ContentBlock::ToolUse(b)) => {
                assert_eq!(a.id, "toolu_0");
                assert_eq!(a.name, "a");
                assert_eq!(b.id, "toolu_1");
                assert_eq!(b.name, "b");
                assert_eq!(b.input, serde_json::json!({"k": 1}));
            }
            other => panic!("expected two tool-use blocks, got {other:?}"),
        }
        assert_eq!(ir.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn response_usage_and_id_mapping() {
        let r = response_with(
            vec![NativePart::Text("x".into())],
            Some(NativeFinishReason::Stop),
        );
        let ir = native_response_to_ir(r).unwrap();
        assert_eq!(ir.usage.input_tokens, 11);
        assert_eq!(ir.usage.output_tokens, 22);
        assert!(ir.usage.cache_creation_input_tokens.is_none());
        assert!(ir.usage.cache_read_input_tokens.is_none());
        assert_eq!(ir.id, "gemini-1.5-pro");
        assert_eq!(ir.model, "gemini-1.5-pro");
        assert_eq!(ir.stop_reason, StopReason::EndTurn);
    }

    // ---- infer_mime_from_url ----

    #[test]
    fn infer_mime_table() {
        let cases = [
            ("a.png", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.JPG", "image/jpeg"),
            ("a.jpeg", "image/jpeg"),
            ("a.gif", "image/gif"),
            ("a.webp", "image/webp"),
            ("a.pdf", "application/pdf"),
            ("a.mp4", "video/mp4"),
            ("a.mp3", "audio/mpeg"),
            ("a.wav", "audio/wav"),
            ("a.unknownext", "application/octet-stream"),
            ("x.png?token=1", "image/png"),
        ];
        for (url, expected) in cases {
            assert_eq!(infer_mime_from_url(url), expected, "url: {url}");
        }
    }

    // ---- map_finish_reason ----

    #[test]
    fn map_finish_reason_table() {
        assert_eq!(
            map_finish_reason(Some(NativeFinishReason::MaxTokens)),
            StopReason::MaxTokens
        );
        assert_eq!(
            map_finish_reason(Some(NativeFinishReason::Safety)),
            StopReason::ContentFilter
        );
        assert_eq!(
            map_finish_reason(Some(NativeFinishReason::Recitation)),
            StopReason::ContentFilter
        );
        assert_eq!(
            map_finish_reason(Some(NativeFinishReason::ToolUse)),
            StopReason::ToolUse
        );
        assert_eq!(
            map_finish_reason(Some(NativeFinishReason::Stop)),
            StopReason::EndTurn
        );
        assert_eq!(
            map_finish_reason(Some(NativeFinishReason::Other)),
            StopReason::EndTurn
        );
        assert_eq!(
            map_finish_reason(Some(NativeFinishReason::FinishReasonUnspecified)),
            StopReason::EndTurn
        );
        assert_eq!(map_finish_reason(None), StopReason::EndTurn);
    }
}
