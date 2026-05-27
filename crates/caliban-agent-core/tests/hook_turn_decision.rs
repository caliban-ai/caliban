//! `after_turn` returns a [`TurnDecision`] so hooks can force continuation
//! or halt the run. `ContinueWith` is capped at `MAX_FORCED_CONTINUATIONS=3`
//! to avoid death-spirals.

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, AgentConfig, Hooks, TurnCtx, TurnDecision, TurnEvent, TurnOutcome,
};
use caliban_provider::{ContentBlock, Message, MockProvider, Role};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

struct ForceContinueHooks {
    count: Arc<AtomicU32>,
}

#[async_trait]
impl Hooks for ForceContinueHooks {
    async fn after_turn(
        &self,
        _ctx: &TurnCtx<'_>,
        _outcome: &TurnOutcome,
    ) -> caliban_agent_core::error::Result<TurnDecision> {
        let n = self.count.fetch_add(1, Ordering::SeqCst);
        if n < 5 {
            Ok(TurnDecision::ContinueWith(vec![Message::user_text(
                "keep going",
            )]))
        } else {
            Ok(TurnDecision::Continue)
        }
    }
}

#[tokio::test]
async fn continue_with_capped_at_three() {
    let mut builder = MockProvider::builder();
    for _ in 0..10 {
        builder = builder.with_response_end_turn("done");
    }
    let provider = builder.build();
    let hooks = Arc::new(ForceContinueHooks {
        count: Arc::new(AtomicU32::new(0)),
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
    let mut final_history = Vec::new();
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { final_messages, .. } = ev {
            final_history = final_messages;
        }
    }
    let injected = final_history
        .iter()
        .filter(|m| {
            m.role == Role::User
                && m.content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text(t) if t.text == "keep going"))
        })
        .count();
    assert_eq!(
        injected, 3,
        "ContinueWith capped at MAX_FORCED_CONTINUATIONS=3"
    );
}
