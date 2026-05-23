#![cfg(feature = "mock")]
#![allow(missing_docs)]

use caliban_provider::{
    CompletionRequest, CompletionResponse, Message, MockProvider, Provider, StopReason,
    StreamEvent, StreamingContentType, StreamingDelta, Usage, collect_message,
};

#[tokio::test]
async fn mock_provider_serves_complete_responses() {
    let mock = MockProvider::new();
    mock.enqueue_complete(Ok(CompletionResponse {
        id: "msg_1".into(),
        model: "mock-model".into(),
        message: Message::assistant_text("hello"),
        stop_reason: StopReason::EndTurn,
        stop_sequence: None,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
    }));

    let req = CompletionRequest::builder("mock-model")
        .user_text("hi")
        .build()
        .unwrap();
    let resp = mock.complete(req).await.unwrap();
    assert_eq!(resp.id, "msg_1");
    assert_eq!(resp.usage.input_tokens, 10);
}

#[tokio::test]
async fn mock_provider_serves_stream() {
    let mock = MockProvider::new();
    mock.enqueue_stream(vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_1".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text("hi".into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage {
                input_tokens: 4,
                output_tokens: 1,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]);

    let req = CompletionRequest::builder("mock-model")
        .user_text("hi")
        .build()
        .unwrap();
    let stream = mock.stream(req).await.unwrap();
    let (msg, stop, usage) = collect_message(stream).await.unwrap();
    assert_eq!(msg.content.len(), 1);
    assert!(matches!(stop, StopReason::EndTurn));
    assert_eq!(usage.input_tokens, 4);
}

#[tokio::test]
async fn collect_message_handles_out_of_order_events() {
    use futures::stream;

    let events = vec![Ok(StreamEvent::Delta {
        index: 0,
        delta: StreamingDelta::Text("orphan".into()),
    })];
    let stream: caliban_provider::MessageStream = Box::pin(stream::iter(events));
    let result = collect_message(stream).await;
    assert!(matches!(
        result,
        Err(caliban_provider::Error::InvalidRequest(_))
    ));
}
