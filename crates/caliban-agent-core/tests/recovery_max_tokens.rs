//! Stage A: one-shot budget escalation when a turn ends in `MaxTokens` with
//! no `tool_use`. Stage B: meta-continuation prompt. Stage C: surrender.

#![allow(missing_docs)]

use std::sync::Arc;

use caliban_agent_core::{Agent, AgentConfig, StopCondition, TurnEvent};
use caliban_provider::{Message, MockProvider};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

fn agent_with(provider: MockProvider, cfg: AgentConfig) -> Arc<Agent> {
    let mut cfg = cfg;
    if cfg.model.is_empty() {
        cfg.model = "mock".into();
    }
    Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .build()
            .expect("agent"),
    )
}

#[tokio::test]
async fn stage_a_escalates_then_succeeds() {
    let provider = MockProvider::builder()
        // turn 1: stop_reason MaxTokens, output 1024 tokens, no tool_use
        .with_response_max_tokens(1024)
        // turn 2 (after Stage A retry with escalated budget): natural EndTurn
        .with_response_end_turn("All done.")
        .build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_tokens_recovery: true,
        escalated_max_tokens: 16_384,
        ..Default::default()
    };
    let agent = agent_with(provider, cfg);

    let mut stream = agent.stream_until_done(
        vec![Message::user_text("write a haiku")],
        CancellationToken::new(),
    );

    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "expected EndOfTurn, got {last_stop:?}"
    );
}

/// PR #68 disabled `max_tokens_recovery` by default because Stage A's
/// silent retry was re-yielding `TurnEnd` and bumping
/// `turns_completed`, inflating the user-visible turn count past the
/// `--max-turns` cap. Stage A is supposed to be invisible to the
/// consumer: a single logical turn that internally re-issues with a
/// larger budget. This test asserts both invariants — exactly one
/// `TurnEnd` event and `turn_count == 1` — for a run that completes
/// successfully via Stage A.
#[tokio::test]
async fn stage_a_retry_does_not_double_count_turn() {
    let provider = MockProvider::builder()
        .with_response_max_tokens(1024)
        .with_response_end_turn("All done.")
        .build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_tokens_recovery: true,
        escalated_max_tokens: 16_384,
        ..Default::default()
    };
    let agent = agent_with(provider, cfg);
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    let mut run_turn_count = 0u32;
    let mut turn_end_count = 0u32;
    while let Some(Ok(ev)) = stream.next().await {
        match ev {
            TurnEvent::TurnEnd { .. } => turn_end_count += 1,
            TurnEvent::RunEnd {
                stopped_for,
                turn_count,
                ..
            } => {
                last_stop = Some(stopped_for);
                run_turn_count = turn_count;
            }
            _ => {}
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "expected EndOfTurn after Stage A success, got {last_stop:?}"
    );
    assert_eq!(
        run_turn_count, 1,
        "Stage A re-issue must not bump turn_count; got {run_turn_count}",
    );
    assert_eq!(
        turn_end_count, 1,
        "exactly one TurnEnd event is observable to the consumer; got {turn_end_count}",
    );
}

/// Characterization (#152): the failed Stage-A attempt's token usage must
/// still be merged into `RunEnd.total_usage`, even though the truncated
/// attempt is otherwise invisible (no `TurnEnd`, no turn-count bump). Guards
/// the `total_usage.merge(turn_usage)` at the Stage-A pre-dispatch path.
///
/// Failed attempt reports `output_tokens = 1024`; the successful retry reports
/// the mock default `output_tokens = 1`. If the failed attempt's usage is
/// dropped, `total_usage.output_tokens` would be `1`; with the merge it is
/// `1025`.
#[tokio::test]
async fn stage_a_failed_attempt_usage_is_billed() {
    let provider = MockProvider::builder()
        .with_response_max_tokens(1024)
        .with_response_end_turn("All done.")
        .build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_tokens_recovery: true,
        escalated_max_tokens: 16_384,
        ..Default::default()
    };
    let agent = agent_with(provider, cfg);
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut total_output = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { total_usage, .. } = ev {
            total_output = Some(total_usage.output_tokens);
        }
    }
    assert_eq!(
        total_output,
        Some(1025),
        "failed Stage-A attempt (1024) + successful retry (1) must both be billed"
    );
}

/// Companion to the regression above: with recovery off, the existing
/// halt-in-one-turn invariant still holds (the catch-all `_` arm in
/// the stop-reason match should not be reachable for Stage A retries).
#[tokio::test]
async fn stage_b_injects_meta_then_continues() {
    let provider = MockProvider::builder()
        // turn 1: MaxTokens (Stage A escalates)
        .with_response_max_tokens(1024)
        // turn 1 retry: still MaxTokens → Stage B fires, inject meta msg
        .with_response_max_tokens(16_384)
        // turn 2: EndTurn after the meta msg
        .with_response_end_turn("Resumed.")
        .build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_tokens_recovery: true,
        max_meta_continuations: 3,
        ..Default::default()
    };
    let agent = agent_with(provider, cfg);
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut final_history = Vec::new();
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd {
            final_messages,
            stopped_for,
            ..
        } = ev
        {
            final_history = final_messages;
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "expected EndOfTurn, got {last_stop:?}"
    );
    let injected = final_history
        .iter()
        .filter(|m| {
            m.role == caliban_provider::Role::User
                && m.content.iter().any(|b| {
                    matches!(b, caliban_provider::ContentBlock::Text(t)
                        if t.text.starts_with("Output token limit hit"))
                })
        })
        .count();
    assert_eq!(
        injected, 1,
        "exactly one meta-continuation message injected"
    );
}

#[tokio::test]
async fn stage_c_surrenders_after_cap() {
    let mut builder = MockProvider::builder();
    // Each "MaxTokens turn" consumes two provider calls (initial hit +
    // Stage A retry). With `max_meta_continuations=3`, the loop runs
    // 4 turns (initial + 3 meta injections) × 2 calls = 8 provider calls
    // before Stage C surrenders. Add a small headroom.
    for _ in 0..10 {
        builder = builder.with_response_max_tokens(16_384);
    }
    let provider = builder.build();

    let cfg = AgentConfig {
        max_tokens: 1024,
        max_tokens_recovery: true,
        max_meta_continuations: 3,
        ..Default::default()
    };
    let agent = agent_with(provider, cfg);
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        matches!(last_stop, Some(StopCondition::MaxTokensExhausted)),
        "expected MaxTokensExhausted, got {last_stop:?}"
    );
}

// ---------------------------------------------------------------------------
// Default-off halt: with recovery disabled (the default), a `MaxTokens` turn
// must end the run in exactly one turn and surface `StopCondition::MaxTokensExhausted`.
// This is the regression test for the LMStudio + Ollama probe finding F6.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_tokens_halts_in_one_turn_when_recovery_off() {
    let provider = MockProvider::builder()
        // Single turn that ends in MaxTokens. If the loop continued past
        // this, the queue would underflow and the test would error out.
        .with_response_max_tokens(8)
        .build();

    let cfg = AgentConfig {
        max_tokens: 8,
        // Explicit even though it's the default — guards against future flips.
        max_tokens_recovery: false,
        ..Default::default()
    };
    let agent = agent_with(provider, cfg);
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    let mut turn_count = 0u32;
    let mut turn_end_count = 0u32;
    while let Some(Ok(ev)) = stream.next().await {
        match ev {
            TurnEvent::TurnEnd { .. } => turn_end_count += 1,
            TurnEvent::RunEnd {
                stopped_for,
                turn_count: tc,
                ..
            } => {
                last_stop = Some(stopped_for);
                turn_count = tc;
            }
            _ => {}
        }
    }
    assert_eq!(turn_count, 1, "expected exactly one turn, got {turn_count}");
    assert_eq!(
        turn_end_count, 1,
        "expected exactly one TurnEnd event, got {turn_end_count}",
    );
    assert!(
        matches!(last_stop, Some(StopCondition::MaxTokensExhausted)),
        "expected MaxTokensExhausted, got {last_stop:?}"
    );
}

// ---------------------------------------------------------------------------
// Other halt paths still terminate cleanly. These guard against the catch-all
// `_` arm in the stop-reason match growing teeth that swallow these reasons.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn end_turn_halts_with_end_of_turn() {
    let provider = MockProvider::builder()
        .with_response_end_turn("done")
        .build();
    let agent = agent_with(provider, AgentConfig::default());
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    let mut turn_count = 0u32;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd {
            stopped_for,
            turn_count: tc,
            ..
        } = ev
        {
            last_stop = Some(stopped_for);
            turn_count = tc;
        }
    }
    assert_eq!(turn_count, 1);
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "expected EndOfTurn, got {last_stop:?}"
    );
}

#[tokio::test]
async fn stop_sequence_halts_with_end_of_turn() {
    let provider = MockProvider::builder()
        .with_response_stop_reason(caliban_provider::StopReason::StopSequence, "halt!")
        .build();
    let agent = agent_with(provider, AgentConfig::default());
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    let mut turn_count = 0u32;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd {
            stopped_for,
            turn_count: tc,
            ..
        } = ev
        {
            last_stop = Some(stopped_for);
            turn_count = tc;
        }
    }
    assert_eq!(turn_count, 1);
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "expected EndOfTurn for StopSequence, got {last_stop:?}"
    );
}

#[tokio::test]
async fn refusal_halts_with_refusal_stop_condition() {
    let provider = MockProvider::builder()
        .with_response_stop_reason(caliban_provider::StopReason::Refusal, "")
        .build();
    let agent = agent_with(provider, AgentConfig::default());
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    let mut turn_count = 0u32;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd {
            stopped_for,
            turn_count: tc,
            ..
        } = ev
        {
            last_stop = Some(stopped_for);
            turn_count = tc;
        }
    }
    assert_eq!(turn_count, 1);
    assert!(
        matches!(last_stop, Some(StopCondition::Refusal(_))),
        "expected Refusal, got {last_stop:?}"
    );
}

#[tokio::test]
async fn content_filter_halts_with_content_filter_stop_condition() {
    let provider = MockProvider::builder()
        .with_response_stop_reason(caliban_provider::StopReason::ContentFilter, "")
        .build();
    let agent = agent_with(provider, AgentConfig::default());
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());

    let mut last_stop = None;
    let mut turn_count = 0u32;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd {
            stopped_for,
            turn_count: tc,
            ..
        } = ev
        {
            last_stop = Some(stopped_for);
            turn_count = tc;
        }
    }
    assert_eq!(turn_count, 1);
    assert!(
        matches!(last_stop, Some(StopCondition::ContentFilter(_))),
        "expected ContentFilter, got {last_stop:?}"
    );
}
