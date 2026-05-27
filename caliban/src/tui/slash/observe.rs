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
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        for line in crate::tui::render_usage_lines(ctx.app) {
            ctx.app.transcript.push(TranscriptLine::Info(line));
        }
        Ok(SlashOutcome::Continue)
    }
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
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        for line in crate::tui::render_context_lines(ctx.app) {
            ctx.app.transcript.push(TranscriptLine::Info(line));
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
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        crate::tui::handle_compact_command(ctx.app);
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
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let deep = args.contains("--deep");
        let diag =
            crate::diagnostics::Diagnostics::run(crate::diagnostics::DiagOpts { deep }).await;
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
