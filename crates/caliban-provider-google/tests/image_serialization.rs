#![allow(missing_docs)]

use caliban_provider::{
    CompletionRequest, ContentBlock, ImageBlock, ImageSource, Message, RequestMetadata, Role,
};
use caliban_provider_google::ir_convert::ir_to_native_request;
use caliban_provider_google::schema::request::NativePart;

fn req_with_image(source: ImageSource) -> CompletionRequest {
    CompletionRequest {
        model: "gemini-1.5-pro".into(),
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
fn google_serializes_base64_image_as_inline_data() {
    let req = req_with_image(ImageSource::Base64 {
        media_type: "image/png".into(),
        data: "AAA".into(),
    });
    let native = ir_to_native_request(req, false).expect("ir_to_native");
    let parts = &native.contents[0].parts;
    let inline = parts
        .iter()
        .find_map(|p| match p {
            NativePart::InlineData(d) => Some(d.clone()),
            _ => None,
        })
        .expect("inline_data part");
    assert_eq!(inline.mime_type, "image/png");
    assert_eq!(inline.data, "AAA");
    let json = serde_json::to_string(&native).expect("serialize");
    assert!(
        json.contains("\"inlineData\""),
        "expected inlineData key, got: {json}"
    );
}

#[test]
fn google_blob_ref_rejected_with_clear_error() {
    let req = req_with_image(ImageSource::BlobRef {
        sha256: "x".repeat(64),
        media_type: "image/png".into(),
    });
    let err = ir_to_native_request(req, false).expect_err("must reject blob ref");
    let s = err.to_string();
    assert!(s.contains("BlobRef"), "msg: {s}");
}
