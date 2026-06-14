#![allow(missing_docs)]

use caliban_provider::{
    CompletionRequest, ContentBlock, ImageBlock, ImageSource, Message, RequestMetadata, Role,
};
use caliban_provider_anthropic::ir_convert::ir_to_native_request;
use caliban_provider_anthropic::schema::request::{
    NativeContent, NativeContentBlock, NativeImageSource,
};

fn req_with_image(source: ImageSource) -> CompletionRequest {
    CompletionRequest {
        model: "claude-3-5-sonnet".into(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source,
                cache_control: None,
                sha256: Some("a".repeat(64)),
                dims: Some((640, 480)),
            })],
        }],
        tools: vec![],
        tool_choice: caliban_provider::ToolChoice::default(),
        max_tokens: 16,
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
fn anthropic_serializes_base64_image_block() {
    let req = req_with_image(ImageSource::Base64 {
        media_type: "image/png".into(),
        data: "AAA".into(),
    });
    let native = ir_to_native_request(req, false);
    let blocks = match &native.messages[0].content {
        NativeContent::Blocks(b) => b,
        NativeContent::Text(_) => panic!("expected blocks"),
    };
    match &blocks[0] {
        NativeContentBlock::Image(img) => match &img.source {
            NativeImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "AAA");
            }
            NativeImageSource::Url { .. } => panic!("expected base64 source, got url"),
        },
        NativeContentBlock::Text(_)
        | NativeContentBlock::ToolUse(_)
        | NativeContentBlock::ToolResult(_)
        | NativeContentBlock::Thinking(_)
        | NativeContentBlock::RedactedThinking { .. } => panic!("expected image block"),
    }
    // Round-trip to JSON; the shape must contain the expected fields.
    let json = serde_json::to_string(&native).expect("serialize");
    assert!(json.contains("\"type\":\"image\""));
    assert!(json.contains("\"media_type\":\"image/png\""));
    assert!(json.contains("\"data\":\"AAA\""));
}

#[test]
fn anthropic_serializes_url_image_block() {
    let req = req_with_image(ImageSource::Url {
        url: "https://example.com/a.png".into(),
    });
    let native = ir_to_native_request(req, false);
    let json = serde_json::to_string(&native).expect("serialize");
    assert!(json.contains("\"type\":\"url\""));
    assert!(json.contains("\"url\":\"https://example.com/a.png\""));
}
