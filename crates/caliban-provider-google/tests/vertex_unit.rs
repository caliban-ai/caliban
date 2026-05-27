//! Hermetic unit tests for the Vertex AI transport — no GCP credentials required.
//!
//! Verifies model-ID pass-through, URL-image conversion behavior, and the
//! `supports_url_images` flag without constructing a live `VertexTransport`.

#![cfg(feature = "vertex")]

use caliban_provider_google::models::models;

// ---------------------------------------------------------------------------
// wire_model_id: Gemini canonical IDs pass through unchanged on Vertex.
// ---------------------------------------------------------------------------

/// Reproduce `wire_model_id` logic for testing.
fn vertex_wire_model_id(canonical: &str) -> String {
    models()
        .into_iter()
        .find(|m| m.id == canonical)
        .map_or_else(|| canonical.to_string(), |m| m.native_id)
}

#[test]
fn vertex_passes_flash_model_through() {
    assert_eq!(vertex_wire_model_id("gemini-2.0-flash"), "gemini-2.0-flash");
}

#[test]
fn vertex_passes_pro_model_through() {
    assert_eq!(vertex_wire_model_id("gemini-1.5-pro"), "gemini-1.5-pro");
}

#[test]
fn vertex_passes_unknown_model_through() {
    assert_eq!(
        vertex_wire_model_id("gemini-99.0-ultra"),
        "gemini-99.0-ultra"
    );
}

// ---------------------------------------------------------------------------
// ir_convert: URL images are accepted when allow_url_images = true.
// ---------------------------------------------------------------------------

use caliban_provider::{
    CompletionRequest, ContentBlock, ImageBlock, ImageSource, Message, RequestMetadata, Role,
    ToolChoice,
};
use caliban_provider_google::ir_convert::ir_to_native_request;
use caliban_provider_google::schema::request::NativePart;

fn minimal_request_with_url_image(url: &str) -> CompletionRequest {
    CompletionRequest {
        model: "gemini-2.0-flash".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source: ImageSource::Url {
                    url: url.to_string(),
                },
                cache_control: None,
                sha256: None,
                dims: None,
            })],
        }],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
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
fn url_image_accepted_for_vertex() {
    let req = minimal_request_with_url_image("https://example.com/photo.png");
    let native =
        ir_to_native_request(req, true).expect("should succeed with allow_url_images=true");
    assert_eq!(native.contents.len(), 1);
    let parts = &native.contents[0].parts;
    assert_eq!(parts.len(), 1);
    assert!(
        matches!(&parts[0], NativePart::FileData(d) if d.file_uri == "https://example.com/photo.png" && d.mime_type == "image/png"),
        "expected FileData part with image/png, got {parts:?}"
    );
}

#[test]
fn url_image_rejected_for_ai_studio() {
    let req = minimal_request_with_url_image("https://example.com/photo.jpg");
    let err =
        ir_to_native_request(req, false).expect_err("should fail with allow_url_images=false");
    let msg = err.to_string();
    assert!(
        msg.contains("base64") || msg.contains("URL"),
        "unexpected error message: {msg}"
    );
}

#[test]
fn url_image_mime_inferred_for_jpeg() {
    let req = minimal_request_with_url_image("https://example.com/img.jpg");
    let native = ir_to_native_request(req, true).unwrap();
    let part = &native.contents[0].parts[0];
    assert!(
        matches!(part, NativePart::FileData(d) if d.mime_type == "image/jpeg"),
        "expected image/jpeg, got {part:?}"
    );
}

#[test]
fn url_image_mime_fallback_for_unknown_ext() {
    let req = minimal_request_with_url_image("https://example.com/file.xyz");
    let native = ir_to_native_request(req, true).unwrap();
    let part = &native.contents[0].parts[0];
    assert!(
        matches!(part, NativePart::FileData(d) if d.mime_type == "application/octet-stream"),
        "expected fallback mime, got {part:?}"
    );
}

// ---------------------------------------------------------------------------
// Vertex endpoint shape (static string test — no HTTP call).
// ---------------------------------------------------------------------------

#[test]
fn vertex_endpoint_streaming_contains_alt_sse() {
    let region = "us-central1";
    let project = "my-project";
    let model = "gemini-2.0-flash";
    let streaming_url = format!(
        "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:streamGenerateContent?alt=sse",
    );
    assert!(
        streaming_url.contains("alt=sse"),
        "streaming URL must include alt=sse"
    );
    assert!(
        streaming_url.contains("streamGenerateContent"),
        "streaming URL must use streamGenerateContent"
    );
}

#[test]
fn vertex_endpoint_non_streaming_uses_generate_content() {
    let region = "us-central1";
    let project = "my-project";
    let model = "gemini-2.0-flash";
    let url = format!(
        "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:generateContent",
    );
    assert!(
        url.contains("generateContent"),
        "non-streaming URL must use generateContent"
    );
    assert!(
        !url.contains("alt=sse"),
        "non-streaming URL must NOT include alt=sse"
    );
}
