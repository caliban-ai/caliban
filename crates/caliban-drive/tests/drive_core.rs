//! Integration tests for the drive core.
//!
//! Every test drives a real `Agent` run over `MockProvider` (no network) and
//! exercises the four drive operations — run / stream / status / input —
//! through the [`DriveSession`] surface with no protocol adapter present.

use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::{Agent, Message, ToolRegistry, TurnEvent};
use caliban_drive::{DriveInbound, DriveOptions, DriveSession, DriveStatus};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Scripted turns
// ---------------------------------------------------------------------------

/// One assistant turn that streams `text` then ends the turn.
fn text_turn(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg".into(),
            model: "mock-model".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.to_string()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn build_agent(mp: Arc<MockProvider>) -> Arc<Agent> {
    let agent = Agent::builder()
        .provider(mp as Arc<dyn Provider + Send + Sync>)
        .tools(ToolRegistry::default())
        .model("mock-model")
        .max_tokens(64)
        .build()
        .expect("agent builds");
    Arc::new(agent)
}

/// Poll `session.status()` until it equals `want`, or panic after a timeout.
async fn await_status(session: &DriveSession, want: &DriveStatus) {
    for _ in 0..500 {
        if &session.status() == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(4)).await;
    }
    panic!(
        "status never reached {want:?}; last observed {:?}",
        session.status()
    );
}

fn variant_names(events: &[TurnEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            TurnEvent::TurnStart { .. } => "TurnStart",
            TurnEvent::AssistantTextDelta { .. } => "AssistantTextDelta",
            TurnEvent::AssistantThinkingDelta { .. } => "AssistantThinkingDelta",
            TurnEvent::ToolCallStart { .. } => "ToolCallStart",
            TurnEvent::ToolCallInputDelta { .. } => "ToolCallInputDelta",
            TurnEvent::ToolCallEnd { .. } => "ToolCallEnd",
            TurnEvent::TurnEnd { .. } => "TurnEnd",
            TurnEvent::RunEnd { .. } => "RunEnd",
        })
        .collect()
}

// ---------------------------------------------------------------------------
// run + stream + status
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_streams_full_turn_sequence_and_ends_done() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_turn("hello"));
    let agent = build_agent(mp);

    let session = DriveSession::spawn(
        agent,
        vec![Message::user_text("hi")],
        DriveOptions::default(),
        &CancellationToken::new(),
    );

    let events: Vec<TurnEvent> = session
        .subscribe()
        .map(|r| r.expect("no stream error"))
        .collect()
        .await;

    let names = variant_names(&events);
    assert!(names.contains(&"TurnStart"), "got {names:?}");
    assert!(names.contains(&"AssistantTextDelta"), "got {names:?}");
    assert!(names.contains(&"TurnEnd"), "got {names:?}");
    assert_eq!(
        names.last(),
        Some(&"RunEnd"),
        "RunEnd must be last: {names:?}"
    );

    assert_eq!(session.wait_done().await, DriveStatus::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_subscriber_replays_whole_run() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_turn("hello"));
    let agent = build_agent(mp);

    let session = DriveSession::spawn(
        agent,
        vec![Message::user_text("hi")],
        DriveOptions::default(),
        &CancellationToken::new(),
    );

    // Let the run fully finish *before* anyone subscribes.
    assert_eq!(session.wait_done().await, DriveStatus::Done);

    let events: Vec<TurnEvent> = session
        .subscribe()
        .map(|r| r.expect("no stream error"))
        .collect()
        .await;

    let names = variant_names(&events);
    assert!(
        names.contains(&"TurnStart"),
        "replay lost TurnStart: {names:?}"
    );
    assert_eq!(
        names.last(),
        Some(&"RunEnd"),
        "replay must include the terminal RunEnd: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_subscribers_see_the_same_events() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_turn("hello"));
    let agent = build_agent(mp);

    let session = DriveSession::spawn(
        agent,
        vec![Message::user_text("hi")],
        DriveOptions::default(),
        &CancellationToken::new(),
    );

    let a = session.subscribe();
    let b = session.subscribe();
    let (ea, eb): (Vec<_>, Vec<_>) = tokio::join!(
        a.map(|r| r.expect("no error")).collect::<Vec<_>>(),
        b.map(|r| r.expect("no error")).collect::<Vec<_>>(),
    );

    assert_eq!(variant_names(&ea), variant_names(&eb));
    assert!(!ea.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_id_is_reported() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_turn("hi"));
    let agent = build_agent(mp);

    let opts = DriveOptions {
        run_id: "run-under-test".into(),
        ..DriveOptions::default()
    };
    let session = DriveSession::spawn(
        agent,
        vec![Message::user_text("hi")],
        opts,
        &CancellationToken::new(),
    );
    assert_eq!(session.id(), "run-under-test");
    let _ = session.wait_done().await;
}

// ---------------------------------------------------------------------------
// input
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_run_awaits_then_resumes_then_ends() {
    let mp = Arc::new(MockProvider::new());
    // Turn 1 (initial prompt), then Turn 2 (after injected input).
    mp.enqueue_stream(text_turn("first"));
    mp.enqueue_stream(text_turn("second"));
    let agent = build_agent(mp);

    let opts = DriveOptions {
        interactive: true,
        ..DriveOptions::default()
    };
    let session = DriveSession::spawn(
        agent,
        vec![Message::user_text("hi")],
        opts,
        &CancellationToken::new(),
    );

    // After the first turn the interactive run parks awaiting input.
    await_status(&session, &DriveStatus::AwaitingInput).await;

    // Inject a follow-up: it resumes and runs the second turn.
    session
        .send_input(DriveInbound::UserMessage {
            text: "continue".into(),
        })
        .expect("interactive session accepts input");

    // It parks again after the second turn.
    await_status(&session, &DriveStatus::AwaitingInput).await;

    // End the conversation.
    session
        .send_input(DriveInbound::EndInput)
        .expect("still accepting input");

    assert_eq!(session.wait_done().await, DriveStatus::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_input_to_noninteractive_session_errors() {
    let mp = Arc::new(MockProvider::new());
    mp.enqueue_stream(text_turn("hi"));
    let agent = build_agent(mp);

    let session = DriveSession::spawn(
        agent,
        vec![Message::user_text("hi")],
        DriveOptions::default(),
        &CancellationToken::new(),
    );
    let err = session
        .send_input(DriveInbound::EndInput)
        .expect_err("non-interactive session rejects input");
    assert!(matches!(err, caliban_drive::DriveError::NotInteractive));
    let _ = session.wait_done().await;
}
