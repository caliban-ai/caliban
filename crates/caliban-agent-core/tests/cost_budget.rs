//! ADR 0058 / #663: agent-loop cost budget. When `AgentConfig::cost_budget_usd`
//! is set *and* a `CostModel` is injected, the loop stops with
//! `StopCondition::CostBudgetExceeded` at the top of the first turn whose
//! accumulated estimated cost reaches the cap. Without a `CostModel` the cap is
//! inert; unset, there is no cap.

#![allow(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::{
    Agent, AgentConfig, ContentBlock, CostModel, StopCondition, TextBlock, Tool, ToolContext,
    ToolError, ToolRegistry, TurnEvent,
};
use caliban_provider::{
    Message, MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta,
    Usage,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

/// A tool-call turn that produces `out` output tokens (so accumulated usage —
/// and therefore priced cost — grows each turn). Calls the read-only `peek`
/// tool, which makes the loop take another turn.
fn tool_use_turn(id: &str, out: u32) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: id.to_owned(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::ToolUse {
                id: format!("tu_{id}"),
                name: "peek".into(),
            },
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::ToolUseInputJson("{}".into()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage_delta: Some(Usage {
                input_tokens: 0,
                output_tokens: out,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

/// A plain text turn that ends the run naturally.
fn text_turn(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "final".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.to_owned()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage::default()),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

struct PeekTool;

#[async_trait]
impl Tool for PeekTool {
    fn name(&self) -> &'static str {
        "peek"
    }
    fn description(&self) -> &'static str {
        "A read-only mock tool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type": "object", "properties": {}}))
    }
    fn is_read_only(&self) -> bool {
        true
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> std::result::Result<Vec<ContentBlock>, ToolError> {
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "peeked".into(),
            cache_control: None,
        })])
    }
}

/// Prices `$1` per accumulated output token — so cost grows deterministically
/// with the run's usage, no rate card required.
struct DollarPerOutputToken;

impl CostModel for DollarPerOutputToken {
    fn cost_usd(&self, usage: &Usage) -> f64 {
        f64::from(usage.output_tokens)
    }
}

fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Arc::new(PeekTool));
    r
}

async fn run(agent: Arc<Agent>) -> (Option<StopCondition>, u32) {
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("go")], CancellationToken::new());
    let mut last_stop = None;
    let mut turns = 0;
    while let Some(item) = stream.next().await {
        match item.expect("no stream error") {
            TurnEvent::TurnEnd { .. } => turns += 1,
            TurnEvent::RunEnd { stopped_for, .. } => last_stop = Some(stopped_for),
            _ => {}
        }
    }
    (last_stop, turns)
}

#[tokio::test]
async fn cost_budget_stops_the_run_once_the_cap_is_reached() {
    // $1/output-token, cap $4. Turn 0 (cost 0 < 4) runs → total 3;
    // turn 1 (cost 3 < 4) runs → total 6; turn 2 top: cost 6 ≥ 4 → fire.
    let provider = MockProvider::new();
    provider.enqueue_stream(tool_use_turn("t0", 3));
    provider.enqueue_stream(tool_use_turn("t1", 3));
    provider.enqueue_stream(tool_use_turn("t2", 3)); // never reached
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider) as Arc<dyn Provider + Send + Sync>)
            .tools(registry())
            .config(AgentConfig {
                model: "mock".into(),
                cost_budget_usd: Some(4.0),
                ..Default::default()
            })
            .cost_model(Arc::new(DollarPerOutputToken))
            .build()
            .expect("agent"),
    );
    let (last_stop, turns) = run(agent).await;
    assert!(
        matches!(last_stop, Some(StopCondition::CostBudgetExceeded(c)) if (c - 4.0).abs() < 1e-9),
        "expected CostBudgetExceeded(4.0), got {last_stop:?}"
    );
    assert_eq!(
        turns, 2,
        "should stop at the turn-2 boundary after two turns"
    );
}

#[tokio::test]
async fn cost_budget_is_inert_without_a_cost_model() {
    // Cap set but no CostModel injected → nothing can price usage, so the run
    // completes normally (the cap is inert).
    let provider = MockProvider::new();
    provider.enqueue_stream(tool_use_turn("t0", 3));
    provider.enqueue_stream(text_turn("done"));
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider) as Arc<dyn Provider + Send + Sync>)
            .tools(registry())
            .config(AgentConfig {
                model: "mock".into(),
                cost_budget_usd: Some(0.001),
                ..Default::default()
            })
            .build()
            .expect("agent"),
    );
    let (last_stop, _) = run(agent).await;
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "no cost model → cap inert → normal completion, got {last_stop:?}"
    );
}

#[tokio::test]
async fn no_cost_budget_is_the_default() {
    let cfg = AgentConfig {
        model: "mock".into(),
        ..Default::default()
    };
    assert_eq!(
        cfg.cost_budget_usd, None,
        "cost_budget_usd defaults to None"
    );
    let provider = MockProvider::new();
    provider.enqueue_stream(text_turn("done"));
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider) as Arc<dyn Provider + Send + Sync>)
            .tools(registry())
            .config(cfg)
            .cost_model(Arc::new(DollarPerOutputToken))
            .build()
            .expect("agent"),
    );
    let (last_stop, _) = run(agent).await;
    assert!(
        matches!(last_stop, Some(StopCondition::EndOfTurn)),
        "no cap → normal completion even with a cost model, got {last_stop:?}"
    );
}
