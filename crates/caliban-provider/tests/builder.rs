#![allow(missing_docs)]
use caliban_provider::{CompletionRequest, Error, Message, RequestMetadata, Role, ToolChoice};

#[test]
fn builder_constructs_valid_request() {
    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .system("You are helpful.")
        .user_text("Hi!")
        .max_tokens(256)
        .build()
        .expect("valid");
    assert_eq!(req.model, "claude-3-5-sonnet");
    assert_eq!(req.messages.len(), 2);
    assert_eq!(req.messages[0].role, Role::System);
    assert_eq!(req.messages[1].role, Role::User);
    assert_eq!(req.max_tokens, 256);
}

#[test]
fn rejects_zero_max_tokens() {
    let err = CompletionRequest::builder("m")
        .user_text("hi")
        .max_tokens(0)
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("max_tokens")));
}

#[test]
fn rejects_empty_model() {
    let err = CompletionRequest::builder("")
        .user_text("hi")
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("model")));
}

#[test]
fn rejects_system_after_user() {
    let req = CompletionRequest {
        model: "m".into(),
        messages: vec![Message::user_text("u"), Message::system_text("s")],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        max_tokens: 64,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: None,
        metadata: RequestMetadata::default(),
    };
    let err = req.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("System")));
}

#[test]
fn rejects_image_in_system() {
    use caliban_provider::{ContentBlock, ImageBlock, ImageSource};
    let req = CompletionRequest {
        model: "m".into(),
        messages: vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Image(ImageBlock {
                    source: ImageSource::Url {
                        url: "https://x/img.png".into(),
                    },
                    cache_control: None,
                })],
            },
            Message::user_text("u"),
        ],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        max_tokens: 64,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: None,
        metadata: RequestMetadata::default(),
    };
    let err = req.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("non-text")));
}

#[test]
fn rejects_no_user_or_assistant() {
    let req = CompletionRequest {
        model: "m".into(),
        messages: vec![Message::system_text("only system")],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        max_tokens: 64,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: None,
        metadata: RequestMetadata::default(),
    };
    let err = req.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(s) if s.contains("User or Assistant")));
}

#[test]
fn multiple_leading_system_messages_ok() {
    let req = CompletionRequest::builder("m")
        .message(Message::system_text("rules"))
        .message(Message::system_text("more rules"))
        .user_text("hi")
        .build()
        .expect("valid");
    assert_eq!(req.messages.len(), 3);
}

#[test]
fn multi_system_preserves_call_order() {
    let req = CompletionRequest::builder("m")
        .system("first")
        .system("second")
        .user_text("u")
        .build()
        .unwrap();
    let texts: Vec<&str> = req
        .messages
        .iter()
        .take(2)
        .map(|m| match &m.content[0] {
            caliban_provider::ContentBlock::Text(t) => t.text.as_str(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(texts, vec!["first", "second"]);
}
