//! Failure-aware hook dispatch: when the run terminates abnormally, the
//! agent should call `after_run_failure` (and NOT `after_run`).

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use caliban_agent_core::{Agent, AgentConfig, Hooks, RunCtx, RunHookOutcome};
use caliban_provider::{Error as ProviderError, Message, MockProvider};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

struct CountingHooks {
    after_run_called: Arc<AtomicU32>,
    after_run_failure_called: Arc<AtomicU32>,
}

#[async_trait]
impl Hooks for CountingHooks {
    async fn after_run(
        &self,
        _: &RunCtx<'_>,
        _: &RunHookOutcome,
    ) -> caliban_agent_core::error::Result<()> {
        self.after_run_called.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn after_run_failure(
        &self,
        _: &RunCtx<'_>,
        _: &RunHookOutcome,
    ) -> caliban_agent_core::error::Result<()> {
        self.after_run_failure_called.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn provider_error_runs_after_run_failure_not_after_run() {
    let provider = MockProvider::builder()
        .with_error_once(ProviderError::Auth("nope".into()))
        .build();
    let hooks = Arc::new(CountingHooks {
        after_run_called: Arc::new(AtomicU32::new(0)),
        after_run_failure_called: Arc::new(AtomicU32::new(0)),
    });
    let cfg = AgentConfig {
        model: "mock".into(),
        ..Default::default()
    };
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .hooks(hooks.clone())
            .build()
            .expect("agent"),
    );
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());
    while stream.next().await.is_some() {}
    assert_eq!(
        hooks.after_run_called.load(Ordering::SeqCst),
        0,
        "after_run must NOT be called on a failure"
    );
    assert_eq!(
        hooks.after_run_failure_called.load(Ordering::SeqCst),
        1,
        "after_run_failure must be called exactly once"
    );
}
