//! IR ↔ `OpenAI` native conversions for request and response.

use caliban_provider::{
    ContentBlock, Error, ImageSource as IrImageSource, Message, Result, Role, StopReason,
    TextBlock as IrTextBlock, Tool as IrTool, ToolChoice as IrToolChoice,
    ToolUseBlock as IrToolUseBlock, Usage as IrUsage,
};

use crate::models::uses_completion_tokens;
use crate::schema::request::{
    NativeContent, NativeContentPart, NativeFunctionCall, NativeImageUrl, NativeMessage,
    NativeReasoning, NativeRequest, NativeStreamOptions, NativeTool, NativeToolCall,
    NativeToolChoice, NativeToolFunction, NativeToolFunctionName,
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

    // Map the IR Effort hint to OpenAI's `reasoning.effort` field. Auto
    // (and a fully-absent field) both omit the block entirely; Low/Med/
    // High pass through verbatim and Max clamps to "high".
    let reasoning = req
        .effort
        .and_then(caliban_provider::Effort::as_openai)
        .map(|level| NativeReasoning {
            effort: level.to_string(),
        });

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
        reasoning,
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
            effort: None,
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
        for model in ["o1", "o3-mini", "o4-mini"] {
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

#[cfg(test)]
mod request_conversion {
    use super::*;
    use caliban_provider::{
        CompletionRequest, ImageBlock, RequestMetadata, ToolResultBlock, ToolUseBlock,
    };

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
            effort: None,
            metadata: RequestMetadata::default(),
        }
    }

    #[test]
    fn leading_system_message_uses_system_role() {
        let mut req = minimal_request("gpt-4o");
        req.messages.insert(0, Message::system_text("be brief"));
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert_eq!(native.messages[0].role, "system");
        assert_eq!(
            native.messages[0].content,
            Some(NativeContent::Text("be brief".into()))
        );
    }

    #[test]
    fn leading_system_message_honors_developer_role() {
        let mut req = minimal_request("o1");
        req.messages.insert(0, Message::system_text("be brief"));
        let native = ir_to_native_request(req, false, "developer").expect("ir_to_native");
        assert_eq!(native.messages[0].role, "developer");
    }

    #[test]
    fn non_leading_system_message_appended() {
        let req = CompletionRequest {
            model: "gpt-4o".into(),
            messages: vec![
                Message::user_text("hello"),
                Message::system_text("mid-stream system"),
            ],
            tools: vec![],
            tool_choice: IrToolChoice::default(),
            max_tokens: 256,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            effort: None,
            metadata: RequestMetadata::default(),
        };
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        // First message is the user; second is the appended system message.
        assert_eq!(native.messages[0].role, "user");
        assert_eq!(native.messages[1].role, "system");
        assert_eq!(
            native.messages[1].content,
            Some(NativeContent::Text("mid-stream system".into()))
        );
    }

    #[test]
    fn user_tool_result_becomes_tool_message() {
        let mut req = minimal_request("gpt-4o");
        req.messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: "call_1".into(),
                content: vec![
                    ContentBlock::Text(IrTextBlock {
                        text: "first".into(),
                        cache_control: None,
                    }),
                    ContentBlock::Text(IrTextBlock {
                        text: "second".into(),
                        cache_control: None,
                    }),
                    ContentBlock::Image(ImageBlock {
                        source: IrImageSource::Url {
                            url: "https://x/y.png".into(),
                        },
                        cache_control: None,
                        sha256: None,
                        dims: None,
                    }),
                ],
                is_error: false,
            })],
        }];
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert_eq!(native.messages.len(), 1);
        let m = &native.messages[0];
        assert_eq!(m.role, "tool");
        assert_eq!(m.tool_call_id, Some("call_1".into()));
        // Text content concatenated with newline; image dropped.
        assert_eq!(m.content, Some(NativeContent::Text("first\nsecond".into())));
    }

    #[test]
    fn user_images_become_image_urls() {
        let mut req = minimal_request("gpt-4o");
        req.messages = vec![Message {
            role: Role::User,
            content: vec![
                ContentBlock::Image(ImageBlock {
                    source: IrImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "abc".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
                ContentBlock::Image(ImageBlock {
                    source: IrImageSource::Url {
                        url: "https://x/y.png".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
                ContentBlock::Image(ImageBlock {
                    source: IrImageSource::BlobRef {
                        sha256: "deadbeef".into(),
                        media_type: "image/png".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
            ],
        }];
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        let parts = match &native.messages[0].content {
            Some(NativeContent::Parts(p)) => p,
            other => panic!("expected Parts, got {other:?}"),
        };
        let urls: Vec<&str> = parts
            .iter()
            .map(|p| match p {
                NativeContentPart::ImageUrl { image_url } => image_url.url.as_str(),
                NativeContentPart::Text { .. } => panic!("unexpected text part"),
            })
            .collect();
        assert_eq!(
            urls,
            vec![
                "data:image/png;base64,abc",
                "https://x/y.png",
                "data:image/png;base64,",
            ]
        );
    }

    #[test]
    fn multi_part_user_content_uses_parts() {
        let mut req = minimal_request("gpt-4o");
        req.messages = vec![Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text(IrTextBlock {
                    text: "look".into(),
                    cache_control: None,
                }),
                ContentBlock::Image(ImageBlock {
                    source: IrImageSource::Url {
                        url: "https://x/y.png".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                }),
            ],
        }];
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert!(matches!(
            native.messages[0].content,
            Some(NativeContent::Parts(_))
        ));
    }

    #[test]
    fn single_text_user_content_collapses_to_text() {
        // minimal_request already has a single text User block.
        let native =
            ir_to_native_request(minimal_request("gpt-4o"), false, "system").expect("ir_to_native");
        assert_eq!(
            native.messages[0].content,
            Some(NativeContent::Text("hi".into()))
        );
    }

    #[test]
    fn assistant_tool_use_becomes_tool_call() {
        let input = serde_json::json!({"q": "weather"});
        let mut req = minimal_request("gpt-4o");
        req.messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text(IrTextBlock {
                    text: "calling".into(),
                    cache_control: None,
                }),
                ContentBlock::ToolUse(ToolUseBlock {
                    id: "call_9".into(),
                    name: "search".into(),
                    input: input.clone(),
                }),
            ],
        }];
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        let m = &native.messages[0];
        assert_eq!(m.tool_calls.len(), 1);
        let tc = &m.tool_calls[0];
        assert_eq!(tc.id, "call_9");
        assert_eq!(tc.kind, "function");
        assert_eq!(tc.function.name, "search");
        assert_eq!(
            tc.function.arguments,
            serde_json::to_string(&input).unwrap()
        );
        assert_eq!(m.content, Some(NativeContent::Text("calling".into())));
    }

    #[test]
    fn assistant_tool_use_only_has_no_content() {
        let mut req = minimal_request("gpt-4o");
        req.messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse(ToolUseBlock {
                id: "call_9".into(),
                name: "search".into(),
                input: serde_json::json!({}),
            })],
        }];
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert_eq!(native.messages[0].content, None);
        assert_eq!(native.messages[0].tool_calls.len(), 1);
    }

    #[test]
    fn tool_choice_variants_map() {
        let mut auto = minimal_request("gpt-4o");
        auto.tool_choice = IrToolChoice::Auto;
        assert_eq!(
            ir_to_native_request(auto, false, "system")
                .unwrap()
                .tool_choice,
            Some(NativeToolChoice::Auto("auto".into()))
        );

        let mut any = minimal_request("gpt-4o");
        any.tool_choice = IrToolChoice::Any;
        assert_eq!(
            ir_to_native_request(any, false, "system")
                .unwrap()
                .tool_choice,
            Some(NativeToolChoice::Auto("required".into()))
        );

        let mut specific = minimal_request("gpt-4o");
        specific.tool_choice = IrToolChoice::Specific {
            name: "search".into(),
        };
        assert_eq!(
            ir_to_native_request(specific, false, "system")
                .unwrap()
                .tool_choice,
            Some(NativeToolChoice::Specific {
                kind: "function".into(),
                function: NativeToolFunctionName {
                    name: "search".into()
                },
            })
        );

        let mut none = minimal_request("gpt-4o");
        none.tool_choice = IrToolChoice::None;
        assert_eq!(
            ir_to_native_request(none, false, "system")
                .unwrap()
                .tool_choice,
            None
        );
    }

    #[test]
    fn tools_map_to_native_functions() {
        let mut req = minimal_request("gpt-4o");
        req.tools = vec![IrTool {
            name: "search".into(),
            description: "search the web".into(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        }];
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert_eq!(native.tools.len(), 1);
        let t = &native.tools[0];
        assert_eq!(t.kind, "function");
        assert_eq!(t.function.name, "search");
        assert_eq!(t.function.description, "search the web");
        assert_eq!(t.function.parameters, serde_json::json!({"type": "object"}));
    }

    #[test]
    fn metadata_user_id_maps_to_native_user() {
        let mut req = minimal_request("gpt-4o");
        req.metadata.user_id = Some("u".into());
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert_eq!(native.user, Some("u".into()));
    }
}

#[cfg(test)]
mod response_conversion {
    use super::*;
    use crate::schema::response::{
        NativeChoice, NativePromptTokensDetails, NativeResponse, NativeResponseMessage, NativeUsage,
    };

    fn response_with(message: NativeResponseMessage, finish: NativeFinishReason) -> NativeResponse {
        NativeResponse {
            id: "resp_1".into(),
            model: "gpt-4o".into(),
            choices: vec![NativeChoice {
                index: 0,
                message,
                finish_reason: finish,
            }],
            usage: NativeUsage::default(),
        }
    }

    fn message(content: Option<&str>) -> NativeResponseMessage {
        NativeResponseMessage {
            role: "assistant".into(),
            content: content.map(Into::into),
            reasoning_content: None,
            tool_calls: Vec::new(),
            refusal: None,
        }
    }

    #[test]
    fn no_choices_is_error() {
        let r = NativeResponse {
            id: "resp_1".into(),
            model: "gpt-4o".into(),
            choices: vec![],
            usage: NativeUsage::default(),
        };
        assert!(native_response_to_ir(r).is_err());
    }

    #[test]
    fn refusal_path_yields_refusal_stop_reason() {
        let mut msg = message(Some("ignored body"));
        msg.refusal = Some("no".into());
        msg.tool_calls = vec![NativeToolCall {
            id: "call_x".into(),
            kind: "function".into(),
            function: NativeFunctionCall {
                name: "search".into(),
                arguments: "{}".into(),
            },
        }];
        let resp = native_response_to_ir(response_with(msg, NativeFinishReason::Stop))
            .expect("conversion");
        assert_eq!(resp.stop_reason, StopReason::Refusal);
        // One Text block with refusal text; tool_calls NOT processed.
        assert_eq!(resp.message.content.len(), 1);
        match &resp.message.content[0] {
            ContentBlock::Text(t) => assert_eq!(t.text, "no"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn finish_reason_maps_to_stop_reason() {
        let cases = [
            (NativeFinishReason::Stop, StopReason::EndTurn),
            (NativeFinishReason::Length, StopReason::MaxTokens),
            (NativeFinishReason::ToolCalls, StopReason::ToolUse),
            (NativeFinishReason::FunctionCall, StopReason::ToolUse),
            (NativeFinishReason::ContentFilter, StopReason::ContentFilter),
        ];
        for (native, expected) in cases {
            let resp =
                native_response_to_ir(response_with(message(Some("hi")), native)).expect("convert");
            assert_eq!(resp.stop_reason, expected, "finish={native:?}");
        }
    }

    #[test]
    fn non_empty_content_yields_text_block() {
        let resp = native_response_to_ir(response_with(
            message(Some("hello")),
            NativeFinishReason::Stop,
        ))
        .expect("convert");
        assert_eq!(resp.message.content.len(), 1);
        match &resp.message.content[0] {
            ContentBlock::Text(t) => assert_eq!(t.text, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn empty_content_yields_no_text_block() {
        let resp =
            native_response_to_ir(response_with(message(Some("")), NativeFinishReason::Stop))
                .expect("convert");
        assert!(resp.message.content.is_empty());
    }

    #[test]
    fn tool_calls_become_tool_use_blocks() {
        let mut msg = message(None);
        msg.tool_calls = vec![NativeToolCall {
            id: "call_7".into(),
            kind: "function".into(),
            function: NativeFunctionCall {
                name: "search".into(),
                arguments: r#"{"q":"x"}"#.into(),
            },
        }];
        let resp = native_response_to_ir(response_with(msg, NativeFinishReason::ToolCalls))
            .expect("convert");
        assert_eq!(resp.message.content.len(), 1);
        match &resp.message.content[0] {
            ContentBlock::ToolUse(tu) => {
                assert_eq!(tu.id, "call_7");
                assert_eq!(tu.name, "search");
                assert_eq!(tu.input, serde_json::json!({"q": "x"}));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn invalid_tool_call_arguments_is_error() {
        let mut msg = message(None);
        msg.tool_calls = vec![NativeToolCall {
            id: "call_7".into(),
            kind: "function".into(),
            function: NativeFunctionCall {
                name: "search".into(),
                arguments: "not json".into(),
            },
        }];
        let r = native_response_to_ir(response_with(msg, NativeFinishReason::ToolCalls));
        assert!(r.is_err());
    }

    #[test]
    fn cache_read_tokens_mapped_when_present() {
        let mut resp = response_with(message(Some("hi")), NativeFinishReason::Stop);
        resp.usage = NativeUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: Some(NativePromptTokensDetails { cached_tokens: 4 }),
        };
        let ir = native_response_to_ir(resp).expect("convert");
        assert_eq!(ir.usage.cache_read_input_tokens, Some(4));
        assert_eq!(ir.usage.input_tokens, 10);
        assert_eq!(ir.usage.output_tokens, 5);
    }

    #[test]
    fn cache_read_zero_tokens_maps_to_none() {
        let mut resp = response_with(message(Some("hi")), NativeFinishReason::Stop);
        resp.usage = NativeUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: Some(NativePromptTokensDetails { cached_tokens: 0 }),
        };
        let ir = native_response_to_ir(resp).expect("convert");
        assert_eq!(ir.usage.cache_read_input_tokens, None);
    }
}

#[cfg(test)]
mod effort_plumbing {
    use super::*;
    use caliban_provider::{CompletionRequest, Effort, RequestMetadata};

    fn build_test_request() -> CompletionRequest {
        CompletionRequest {
            model: "gpt-5".into(),
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
            effort: None,
            metadata: RequestMetadata::default(),
        }
    }

    #[test]
    fn effort_low_sets_reasoning_effort_low() {
        let mut req = build_test_request();
        req.effort = Some(Effort::Low);
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        let reasoning = native.reasoning.expect("reasoning block emitted");
        assert_eq!(reasoning.effort, "low");
    }

    #[test]
    fn effort_max_clamps_to_high() {
        let mut req = build_test_request();
        req.effort = Some(Effort::Max);
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        let reasoning = native.reasoning.expect("reasoning block emitted");
        assert_eq!(reasoning.effort, "high");
    }

    #[test]
    fn effort_auto_omits_reasoning_field() {
        let mut req = build_test_request();
        req.effort = Some(Effort::Auto);
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert!(
            native.reasoning.is_none(),
            "reasoning field omitted on Auto"
        );
    }

    #[test]
    fn effort_unset_omits_reasoning_field() {
        let req = build_test_request();
        let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
        assert!(native.reasoning.is_none(), "reasoning omitted when unset");
    }
}
