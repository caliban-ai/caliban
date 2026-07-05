//! Observability / cost commands: `/usage`, `/context`, `/compact`,
//! `/doctor`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::{Overlay, TranscriptLine};

/// `/usage` — cumulative tokens + USD per model (ADR 0033).
pub(crate) struct UsageCommand;

#[async_trait]
impl SlashCommand for UsageCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/usage",
            description: "show token + cost usage for this session",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let app = &mut *ctx.app;
        let mut lines = format_usage_lines(&app.cost_accumulator);
        if let Some(sess) = app.session.as_ref() {
            lines.push(format!(
                "  session {} \u{2014} {} turns, {} input + {} output tokens",
                sess.name,
                sess.turn_count(),
                sess.total_usage.input_tokens,
                sess.total_usage.output_tokens,
            ));
        }
        for line in lines {
            app.transcript.push(TranscriptLine::Info(line));
        }
        Ok(SlashOutcome::Continue)
    }
}

/// Pure formatter for `/usage`. Split out so the rendering is unit-testable
/// without constructing a full `App`. (Lives with the `/usage` command per
/// ADR 0040; previously a free function in `tui/events.rs`.)
pub(crate) fn format_usage_lines(cost: &caliban_telemetry::CostAccumulator) -> Vec<String> {
    let bd = cost.breakdown();
    let mut lines = vec![format!(
        "usage \u{2014} total ${:.4}",
        rust_decimal::prelude::ToPrimitive::to_f64(&bd.total_usd).unwrap_or(0.0),
    )];
    if bd.by_model.is_empty() {
        lines.push("  (no provider calls yet this session)".into());
    } else {
        lines.push("  by model:".into());
        for mc in &bd.by_model {
            let usd_f = rust_decimal::prelude::ToPrimitive::to_f64(&mc.usd).unwrap_or(0.0);
            lines.push(format!(
                "    {}/{}  in {}  out {}  cache_r {}  cache_w {}  ${:.4}",
                mc.provider,
                mc.model,
                mc.input_tokens,
                mc.output_tokens,
                mc.cache_read_tokens,
                mc.cache_creation_tokens,
                usd_f,
            ));
        }
    }
    if bd.cache_savings_usd > rust_decimal::Decimal::ZERO {
        let sav = rust_decimal::prelude::ToPrimitive::to_f64(&bd.cache_savings_usd).unwrap_or(0.0);
        lines.push(format!("  cache savings vs no-cache: ${sav:.4}"));
    }
    lines
}

/// `/context` — context window utilization (ADR 0033) + top-N largest
/// content blocks for spotting the noisiest tool results.
pub(crate) struct ContextCommand;

#[async_trait]
impl SlashCommand for ContextCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/context",
            description: "show context window utilization + top-N largest blocks",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        for line in format_context_lines(&ctx.app.context_window) {
            ctx.app.transcript.push(TranscriptLine::Info(line));
        }
        // ADR-0046: when lazy_mcp is enabled, surface the activation set.
        // Omitted when off — every MCP tool is always present so the
        // line would carry no information.
        let cfg = ctx.app.agent.config();
        if cfg.lazy_mcp {
            let active_guard = ctx.app.agent.mcp_active();
            let snap = active_guard.load();
            ctx.app.transcript.push(TranscriptLine::Info(format!(
                "MCP active: {}/{}",
                snap.len(),
                cfg.max_active_schemas,
            )));
            for name in snap.iter_active() {
                ctx.app
                    .transcript
                    .push(TranscriptLine::Info(format!("  {name}")));
            }
        }
        // Top-N largest blocks across the in-memory history. Reaches
        // into messages directly because `ContextWindow` only retains
        // per-kind totals, not per-block sizes.
        let top = context_breakdown::top_n_blocks(&ctx.app.messages, 5);
        if !top.is_empty() {
            ctx.app.transcript.push(TranscriptLine::Info(
                "largest blocks (descending chars):".into(),
            ));
            for (i, b) in top.iter().enumerate() {
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "  {:>2}. {:<14} {:>8} chars  {}",
                    i + 1,
                    b.kind,
                    b.chars,
                    b.label,
                )));
            }
        }
        Ok(SlashOutcome::Continue)
    }
}

/// Pure formatter for `/context`. Split out so the rendering is unit-testable
/// without constructing a full `App`. (Lives with the `/context` command per
/// ADR 0040; previously a free function in `tui/events.rs`.)
pub(crate) fn format_context_lines(window: &caliban_telemetry::ContextWindow) -> Vec<String> {
    let bd = window.breakdown();
    let pct = if bd.capacity == 0 {
        0
    } else {
        // utilization_bp is 0..=10_000 (bp); convert to percent.
        u32::from(window.utilization_bp()) / 100
    };
    let mut lines = Vec::new();
    if bd.capacity == 0 {
        lines.push(
            "context window \u{2014} no capacity reported by provider (start a turn first)".into(),
        );
        return lines;
    }
    lines.push(format!(
        "context window \u{2014} {}-token window, {pct}% used ({} of {})",
        bd.capacity, bd.used, bd.capacity,
    ));
    let mut bins: Vec<_> = bd.bins.iter().filter(|b| b.tokens > 0).collect();
    bins.sort_by_key(|b| std::cmp::Reverse(b.tokens));
    for b in &bins {
        lines.push(format!("  {:<18} {:>8}", b.kind.label(), b.tokens));
    }
    if bins.is_empty() {
        lines.push("  (no messages yet)".into());
    }
    if pct >= 80 {
        lines.push("  warning: \u{2265} 80% of context used \u{2014} consider /compact".into());
    }
    lines
}

/// Pure helpers for `/context` so the largest-block computation is
/// unit-testable without an `App`.
pub(crate) mod context_breakdown {
    use caliban_provider::{ContentBlock, Message};

    /// One entry in the top-N largest-blocks list.
    #[derive(Debug, Clone)]
    pub(crate) struct BlockEntry {
        /// Short kind label (`"text"`, `"tool_use"`, `"tool_result"`, …).
        pub(crate) kind: &'static str,
        /// Human-readable label (tool name, first-N chars of text, …).
        pub(crate) label: String,
        /// Length in chars (proxy for tokens).
        pub(crate) chars: usize,
    }

    /// Return the `n` largest content blocks across `messages`, sorted
    /// descending by char-count.
    #[must_use]
    pub(crate) fn top_n_blocks(messages: &[Message], n: usize) -> Vec<BlockEntry> {
        let mut entries: Vec<BlockEntry> = Vec::new();
        for m in messages {
            for cb in &m.content {
                match cb {
                    ContentBlock::Text(t) => {
                        let first_line = t
                            .text
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(60)
                            .collect::<String>();
                        entries.push(BlockEntry {
                            kind: "text",
                            label: first_line,
                            chars: t.text.chars().count(),
                        });
                    }
                    ContentBlock::ToolUse(u) => {
                        entries.push(BlockEntry {
                            kind: "tool_use",
                            label: u.name.clone(),
                            chars: u.input.to_string().chars().count(),
                        });
                    }
                    ContentBlock::ToolResult(r) => {
                        // Concatenate text-content chars; ignore images.
                        let chars: usize = r
                            .content
                            .iter()
                            .filter_map(|c| match c {
                                ContentBlock::Text(t) => Some(t.text.chars().count()),
                                _ => None,
                            })
                            .sum();
                        entries.push(BlockEntry {
                            kind: "tool_result",
                            label: r.tool_use_id.chars().take(20).collect::<String>(),
                            chars,
                        });
                    }
                    ContentBlock::Thinking(t) => {
                        entries.push(BlockEntry {
                            kind: "thinking",
                            label: t.thinking.chars().take(60).collect::<String>(),
                            chars: t.thinking.chars().count(),
                        });
                    }
                    ContentBlock::Image(_) => {
                        entries.push(BlockEntry {
                            kind: "image",
                            label: "(image)".into(),
                            chars: 0,
                        });
                    }
                }
            }
        }
        entries.sort_by_key(|b| std::cmp::Reverse(b.chars));
        entries.truncate(n);
        entries
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn top_n_returns_largest_descending() {
            let msgs = vec![
                Message::user_text("small"),
                Message::user_text("x".repeat(10_000)),
                Message::assistant_text("y".repeat(20_000)),
            ];
            let top = top_n_blocks(&msgs, 2);
            assert_eq!(top.len(), 2);
            assert!(top[0].chars >= top[1].chars);
            assert_eq!(top[0].chars, 20_000);
        }

        #[test]
        fn top_n_n_larger_than_history_returns_all() {
            let msgs = vec![Message::user_text("hi"), Message::assistant_text("there")];
            let top = top_n_blocks(&msgs, 100);
            assert_eq!(top.len(), 2);
        }
    }
}

/// `/compact` — trigger the configured Compactor (ADR 0033).
pub(crate) struct CompactCommand;

#[async_trait]
impl SlashCommand for CompactCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/compact",
            description: "trigger the configured compactor; reports dropped/summarized count",
            args_hint: "",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let app = &mut *ctx.app;
        if app.messages.is_empty() {
            app.transcript.push(TranscriptLine::Info(
                "compact: no messages to compact".into(),
            ));
            return Ok(SlashOutcome::Continue);
        }
        let model = app.args.model.clone().unwrap_or_else(|| {
            crate::default_model_for(crate::resolved_provider(&app.args)).to_string()
        });
        let caps = app.agent.provider().capabilities(&model);
        let before = caliban_agent_core::estimate_tokens(&app.messages);
        let before_count = app.messages.len();
        let compactor = app.agent.compactor();
        let messages = app.messages.clone();
        let result = compactor.compact(&messages, &caps).await;
        match result {
            Err(e) => app
                .transcript
                .push(TranscriptLine::Error(format!("compact failed: {e}"))),
            Ok(None) => app.transcript.push(TranscriptLine::Info(format!(
                "compact: no-op (strategy {} kept {before_count} messages, ~{before} tokens)",
                compactor.strategy_name(),
            ))),
            Ok(Some(compaction)) => {
                let new = compaction.messages;
                let after = caliban_agent_core::estimate_tokens(&new);
                let after_count = new.len();
                let dropped = before_count.saturating_sub(after_count);
                app.messages.clone_from(&new);
                if let Some(sess) = app.session.as_mut() {
                    sess.messages.clone_from(&new);
                }
                // Refresh context window from the post-compact history.
                app.context_window.record_history(&new);
                app.transcript.push(TranscriptLine::Info(format!(
                    "compact (strategy {}): {before_count} \u{2192} {after_count} messages \
                     ({dropped} dropped/summarized), ~{before} \u{2192} ~{after} tokens",
                    compactor.strategy_name(),
                )));
            }
        }
        Ok(SlashOutcome::Continue)
    }
}

/// `/doctor` — run health checks: settings parse, MCP reachability,
/// skills loaded, hooks parse, provider auth, workspace permissions.
/// Prints pass/fail per check.
pub(crate) struct DoctorCommand;

#[async_trait]
impl SlashCommand for DoctorCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/doctor",
            description: "run startup-time health checks (skills, hooks, MCP, auth)",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let deep = args.contains("--deep");
        let diag = crate::diagnostics::Diagnostics::run(crate::diagnostics::DiagOpts {
            deep,
            model: None,
        })
        .await;
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "doctor \u{2014} {} check(s):",
            diag.checks.len()
        )));
        for c in &diag.checks {
            ctx.app.transcript.push(TranscriptLine::Info(format!(
                "  {} {} \u{2014} {}",
                c.status.glyph(),
                c.name,
                c.hint,
            )));
        }
        Ok(SlashOutcome::Continue)
    }
}

// The legacy `pub(crate) mod doctor` (skills/hooks/mcp/provider/workspace
// checks) was replaced by the shared `crate::diagnostics` module in
// Plan C Task 7 so the TUI `/doctor` slash command and the headless
// `caliban doctor` subcommand call the same runner.

/// `/system` — show the active system prompt overlay.
pub(crate) struct SystemCommand;

#[async_trait]
impl SlashCommand for SystemCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/system",
            description: "view the active system prompt",
            args_hint: "",
            hidden: true, // present for backwards compat; not in spec.
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::Overlay(Overlay::System))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(UsageCommand));
    registry.register(Arc::new(ContextCommand));
    registry.register(Arc::new(CompactCommand));
    registry.register(Arc::new(DoctorCommand));
    registry.register(Arc::new(SystemCommand));
}
