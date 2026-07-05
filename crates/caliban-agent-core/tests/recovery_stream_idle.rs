//! Stream-idle watchdog: when the provider stays silent past
//! `stream_idle_timeout_ms`, the run terminates with
//! `StopCondition::StreamIdle`.

#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::{Agent, AgentConfig, StopCondition, TurnEvent};
use caliban_provider::{Message, MockProvider};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

/// #330: a provider whose `stream()` *call itself* hangs (server accepts the
/// connection but never sends response headers) must be bounded by the
/// agent-core first-byte timeout — not hang forever. This is distinct from the
/// silent-stream cases: there `stream()` returns and the *stream* is silent (a
/// `WatchedStream` prefill/idle catch); here `.send()` never resolves, *before*
/// any stream watchdog can observe.
#[tokio::test]
async fn first_byte_timeout_bounds_hanging_stream_call() {
    let provider = MockProvider::builder()
        // Never returns headers within any sane budget.
        .with_hanging_stream(Duration::from_secs(30))
        .build();
    let cfg = AgentConfig {
        model: "mock".into(),
        // The prefill budget doubles as the first-byte budget (#330).
        stream_prefill_timeout_ms: 100,
        stream_idle_timeout_ms: 100,
        ..Default::default()
    };
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .build()
            .expect("agent"),
    );
    let start = std::time::Instant::now();
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "first-byte timeout should abort at ~100ms, not hang: {:?}",
        start.elapsed()
    );
    assert!(
        matches!(last_stop, Some(StopCondition::StreamIdle(_))),
        "expected StreamIdle from the first-byte timeout, got {last_stop:?}"
    );
}

#[tokio::test]
async fn stream_idle_aborts_run() {
    let provider = MockProvider::builder()
        .with_silent_stream(Duration::from_secs(10))
        .build();
    let cfg = AgentConfig {
        model: "mock".into(),
        stream_idle_timeout_ms: 200,
        // A silent stream never yields a first chunk, so it lives in the
        // *prefill* phase (#263); bound the prefill budget too or it would
        // wait the 300s default before aborting.
        stream_prefill_timeout_ms: 200,
        ..Default::default()
    };
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .build()
            .expect("agent"),
    );
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::StreamIdle(_))),
        "expected StreamIdle, got {last_stop:?}"
    );
}

/// #263: a stream that delays its first chunk past the (tight) idle window but
/// within the (generous) prefill budget must NOT abort — it completes. Guards
/// the prefill grace end-to-end through the agent loop.
#[tokio::test]
async fn slow_prefill_within_budget_completes() {
    let provider = MockProvider::builder()
        .with_delayed_first_chunk(Duration::from_millis(120), "done")
        .build();
    let cfg = AgentConfig {
        model: "mock".into(),
        stream_idle_timeout_ms: 40,       // tight mid-content window
        stream_prefill_timeout_ms: 5_000, // generous prefill budget
        ..Default::default()
    };
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .build()
            .expect("agent"),
    );
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    // Positive assertions (#334): a bare `!matches!(last_stop, StreamIdle)`
    // passes vacuously when the run errors and leaves `last_stop = None`. Fail
    // on any stream `Err`, and assert the turn actually completed *and* streamed
    // its content — not merely "didn't trip the idle watchdog".
    let mut last_stop = None;
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        match item.expect("within-budget prefill run must not yield a stream error") {
            TurnEvent::AssistantTextDelta { text: fragment, .. } => text.push_str(&fragment),
            TurnEvent::RunEnd { stopped_for, .. } => last_stop = Some(stopped_for),
            _ => {}
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "slow prefill within budget should complete with EndOfTurn, got {last_stop:?}",
    );
    assert_eq!(
        text, "done",
        "the delayed first chunk should stream through to completion",
    );
}
