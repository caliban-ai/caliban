//! Routing helpers — strict-by-default capability filtering and the
//! text-fallback rewrite when strict routing is disabled.
//!
//! When `CALIBAN_STRICT_ROUTING=false`, an image-bearing request that hits
//! a non-vision route has its image blocks replaced with the text
//! placeholder:
//!
//! ```text
//! [image attached — provider does not support vision; dims: 1024x768]
//! ```

use caliban_provider::{CompletionRequest, ContentBlock, ImageSource, TextBlock};

// Required for the default RequestMetadata access in tests below.
#[cfg(test)]
use caliban_provider::RequestMetadata;

/// Returns `true` if strict routing is enabled.
///
/// The default (env unset, empty, `true`, `1`, `yes`) is **strict**; any of
/// `false` / `0` / `no` disables strictness.
#[must_use]
pub fn strict_routing_enabled() -> bool {
    std::env::var("CALIBAN_STRICT_ROUTING").map_or(true, |v| {
        !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | "no")
    })
}

/// Walk every `ContentBlock::Image` in `req` and replace it with a
/// `ContentBlock::Text` placeholder. Returns the number of substitutions.
#[allow(clippy::implicit_hasher)]
pub fn rewrite_for_text_fallback(req: &mut CompletionRequest) -> u32 {
    let mut count: u32 = 0;
    for msg in &mut req.messages {
        for block in &mut msg.content {
            if let ContentBlock::Image(img) = block {
                let dims = img.dims.unwrap_or((0, 0));
                let kind = match &img.source {
                    ImageSource::Base64 { media_type, .. }
                    | ImageSource::BlobRef { media_type, .. } => media_type.clone(),
                    ImageSource::Url { .. } => "image/url".into(),
                };
                let text = format!(
                    "[image attached — provider does not support vision; \
                     dims: {}x{}; mime: {kind}]",
                    dims.0, dims.1
                );
                *block = ContentBlock::Text(TextBlock {
                    text,
                    cache_control: None,
                });
                count = count.saturating_add(1);
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{
        CompletionRequest, ContentBlock, ImageBlock, ImageSource, Message, Role, ToolChoice,
    };

    fn req_with_image() -> CompletionRequest {
        CompletionRequest {
            model: "x".into(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Image(ImageBlock {
                    source: ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "abc".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: Some((640, 480)),
                })],
            }],
            tools: vec![],
            tool_choice: ToolChoice::default(),
            max_tokens: 16,
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
    fn text_fallback_replaces_image_with_placeholder() {
        let mut req = req_with_image();
        let n = rewrite_for_text_fallback(&mut req);
        assert_eq!(n, 1);
        match &req.messages[0].content[0] {
            ContentBlock::Text(t) => {
                assert!(t.text.contains("640x480"));
                assert!(t.text.contains("image/png"));
                assert!(t.text.contains("does not support vision"));
            }
            other => panic!("expected text fallback, got {other:?}"),
        }
    }

    #[test]
    fn text_fallback_is_zero_when_no_images() {
        let mut req = CompletionRequest {
            model: "x".into(),
            messages: vec![Message::user_text("hello")],
            tools: vec![],
            tool_choice: ToolChoice::default(),
            max_tokens: 16,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            effort: None,
            metadata: RequestMetadata::default(),
        };
        let n = rewrite_for_text_fallback(&mut req);
        assert_eq!(n, 0);
    }

    #[test]
    #[allow(
        unsafe_code,
        reason = "test mutates process env around CALIBAN_STRICT_ROUTING"
    )]
    fn strict_routing_default_is_strict() {
        // SAFETY: tests run sequentially within a process; clean up after.
        unsafe { std::env::remove_var("CALIBAN_STRICT_ROUTING") };
        assert!(strict_routing_enabled());
    }

    #[test]
    #[allow(
        unsafe_code,
        reason = "test mutates process env around CALIBAN_STRICT_ROUTING"
    )]
    fn strict_routing_can_be_disabled() {
        // SAFETY: see above; we restore the prior state immediately after.
        unsafe { std::env::set_var("CALIBAN_STRICT_ROUTING", "false") };
        assert!(!strict_routing_enabled());
        unsafe { std::env::remove_var("CALIBAN_STRICT_ROUTING") };
    }
}
