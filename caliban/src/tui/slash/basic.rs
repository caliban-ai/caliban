//! Basic session-control commands: `/clear`, `/help`, `/quit`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::Overlay;

/// `/clear` — clear the current session's message history; keep the system
/// prompt, todos, plan-mode, and skills cache.
pub(crate) struct ClearCommand;

#[async_trait]
impl SlashCommand for ClearCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/clear",
            description: "clear the transcript and conversation history",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        ctx.app.transcript.clear();
        ctx.app.messages.clear();
        ctx.app.last_turn_ttft_ms = None;
        if let Some(sess) = ctx.app.session.as_mut() {
            sess.messages.clear();
        }
        Ok(SlashOutcome::Continue)
    }
}

/// `/help` — open the help overlay (lists every visible registered
/// command, sourced from the registry). Renders with `slash_help_lines`.
pub(crate) struct HelpCommand;

#[async_trait]
impl SlashCommand for HelpCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/help",
            description: "list available slash commands",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::Overlay(Overlay::SlashHelp))
    }
}

/// `/quit` (and alias `/exit`) — exit caliban.
pub(crate) struct QuitCommand {
    name: &'static str,
}

#[async_trait]
impl SlashCommand for QuitCommand {
    fn meta(&self) -> &SlashCommandMeta {
        // Allocate a `SlashCommandMeta` per call so the `name` field can
        // vary between aliases without splitting impls.
        match self.name {
            "/exit" => &SlashCommandMeta {
                name: "/exit",
                description: "exit caliban",
                args_hint: "",
                hidden: true, // hide the alias from /help; /quit is canonical.
            },
            _ => &SlashCommandMeta {
                name: "/quit",
                description: "exit caliban",
                args_hint: "",
                hidden: false,
            },
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::Quit)
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ClearCommand));
    registry.register(Arc::new(HelpCommand));
    registry.register(Arc::new(QuitCommand { name: "/quit" }));
    registry.register(Arc::new(QuitCommand { name: "/exit" }));
}
