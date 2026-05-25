#![allow(missing_docs)]
use caliban_provider::{
    CacheControl, ContentBlock, ImageBlock, ImageSource, Message, Role, TextBlock,
};
use proptest::prelude::*;

fn arb_text_block() -> impl Strategy<Value = TextBlock> {
    (
        any::<String>(),
        prop::option::of(Just(CacheControl::Ephemeral)),
    )
        .prop_map(|(text, cache_control)| TextBlock {
            text,
            cache_control,
        })
}

fn arb_image_block() -> impl Strategy<Value = ImageBlock> {
    (
        any::<String>(),
        any::<String>(),
        prop::option::of(Just(CacheControl::Ephemeral)),
    )
        .prop_map(|(mime, data, cache_control)| ImageBlock {
            source: ImageSource::Base64 {
                media_type: mime,
                data,
            },
            cache_control,
            sha256: None,
            dims: None,
        })
}

fn arb_content_block() -> impl Strategy<Value = ContentBlock> {
    prop_oneof![
        arb_text_block().prop_map(ContentBlock::Text),
        arb_image_block().prop_map(ContentBlock::Image),
    ]
}

fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![Just(Role::User), Just(Role::Assistant), Just(Role::System)]
}

fn arb_message() -> impl Strategy<Value = Message> {
    (arb_role(), prop::collection::vec(arb_content_block(), 0..3))
        .prop_map(|(role, content)| Message { role, content })
}

proptest! {
    #[test]
    fn message_serde_round_trip(m in arb_message()) {
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(parsed, m);
    }
}

#[test]
fn image_source_url_round_trips() {
    use caliban_provider::{ContentBlock, ImageBlock, ImageSource};
    let cb = ContentBlock::Image(ImageBlock {
        source: ImageSource::Url {
            url: "https://example.com/img.png".to_string(),
        },
        cache_control: None,
        sha256: None,
        dims: None,
    });
    let json = serde_json::to_string(&cb).expect("serializes");
    assert!(json.contains("\"url\":\"https://example.com/img.png\""));
    let parsed: ContentBlock = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(parsed, cb);
}
