//! `/cost` — print the session's cumulative USD spend with a
//! per-(provider, model) breakdown.
//!
//! Sourced from `App.cost_accumulator` (always present per ADR 0033).
//! Until the `Overlay` enum gains a non-`Copy` variant, the breakdown is
//! emitted inline as `TranscriptLine::Info` rows — same display shape as
//! `/usage`, just with USD as the leading axis.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::TranscriptLine;

/// `/cost` — show cumulative cost + per-model breakdown.
pub(crate) struct CostCommand;

#[async_trait]
impl SlashCommand for CostCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/cost",
            description: "show cumulative cost and per-model breakdown",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }

    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let breakdown = ctx.app.cost_accumulator.breakdown();
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "cost \u{2014} total: ${:.4}  cache savings: ${:.4}",
            breakdown.total_usd, breakdown.cache_savings_usd,
        )));
        if breakdown.by_model.is_empty() {
            ctx.app.transcript.push(TranscriptLine::Info(
                "(no usage recorded yet \u{2014} send a prompt first)".into(),
            ));
        } else {
            for row in &breakdown.by_model {
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "  {} / {}  in={}  out={}  cache_r={}  cache_w={}  ${:.4}",
                    row.provider,
                    row.model,
                    row.input_tokens,
                    row.output_tokens,
                    row.cache_read_tokens,
                    row.cache_creation_tokens,
                    row.usd,
                )));
            }
        }
        Ok(SlashOutcome::Continue)
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(CostCommand));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use caliban_provider::{RequestPurpose, Usage};

    #[tokio::test]
    async fn cost_command_emits_total_line_even_when_empty() {
        let mut app = App::for_tests();
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = CostCommand.execute("", &mut ctx).await.unwrap();
        assert!(matches!(outcome, SlashOutcome::Continue));
        let body: String = app
            .transcript
            .iter()
            .filter_map(|l| match l {
                TranscriptLine::Info(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains("cost"), "missing leading cost line: {body}");
    }

    #[tokio::test]
    async fn cost_command_lists_per_model_rows_after_record() {
        let mut app = App::for_tests();
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 2_000,
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        };
        let _ = app.cost_accumulator.record(
            "anthropic",
            "claude-sonnet-4-6",
            &usage,
            Some(RequestPurpose::MainLoop),
        );
        let mut ctx = app.slash_ctx_for_tests();
        let _ = CostCommand.execute("", &mut ctx).await.unwrap();
        let body: String = app
            .transcript
            .iter()
            .filter_map(|l| match l {
                TranscriptLine::Info(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("claude-sonnet-4-6"),
            "missing model row: {body}"
        );
    }
}
