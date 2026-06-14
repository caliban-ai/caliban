#![allow(missing_docs)]

use caliban_provider::{
    CompletionRequest, ContentBlock, ImageBlock, ImageSource, Message, RequestMetadata, Role,
};
use caliban_provider_openai::ir_convert::ir_to_native_request;
use caliban_provider_openai::schema::request::{NativeContent, NativeContentPart};

fn req_with_image(source: ImageSource) -> CompletionRequest {
    CompletionRequest {
        model: "gpt-4o".into(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source,
                cache_control: None,
                sha256: None,
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
fn openai_serializes_base64_image_as_data_url() {
    let req = req_with_image(ImageSource::Base64 {
        media_type: "image/png".into(),
        data: "AAA".into(),
    });
    let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
    let user_msg = native
        .messages
        .iter()
        .find(|m| m.role == "user")
        .expect("user message");
    let parts = match &user_msg.content {
        Some(NativeContent::Parts(p)) => p,
        other => panic!("expected parts, got {other:?}"),
    };
    let img_url = parts
        .iter()
        .find_map(|p| match p {
            NativeContentPart::ImageUrl { image_url } => Some(image_url.url.clone()),
            NativeContentPart::Text { .. } => None,
        })
        .expect("image_url part");
    assert_eq!(img_url, "data:image/png;base64,AAA");
}

#[test]
fn openai_serializes_url_image_as_https() {
    let req = req_with_image(ImageSource::Url {
        url: "https://example.com/a.png".into(),
    });
    let native = ir_to_native_request(req, false, "system").expect("ir_to_native");
    let user_msg = native
        .messages
        .iter()
        .find(|m| m.role == "user")
        .expect("user message");
    let parts = match &user_msg.content {
        Some(NativeContent::Parts(p)) => p,
        other => panic!("expected parts, got {other:?}"),
    };
    let img_url = parts
        .iter()
        .find_map(|p| match p {
            NativeContentPart::ImageUrl { image_url } => Some(image_url.url.clone()),
            NativeContentPart::Text { .. } => None,
        })
        .expect("image_url part");
    assert_eq!(img_url, "https://example.com/a.png");
}
