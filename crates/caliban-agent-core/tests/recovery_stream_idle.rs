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
