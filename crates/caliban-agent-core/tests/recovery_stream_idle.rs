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

    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        !matches!(last_stop, Some(StopCondition::StreamIdle(_))),
        "slow prefill within budget must not trip the idle watchdog, got {last_stop:?}",
    );
}
