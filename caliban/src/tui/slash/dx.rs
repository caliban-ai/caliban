//! Diagnostics / DX commands: `/rewind`, `/heapdump`, `/feedback`,
//! `/loop`, `/statusline`, `/tui`, `/voice`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::{Overlay, TranscriptLine};

/// `/rewind` — open the checkpoint picker overlay (ADR 0028).
pub(crate) struct RewindCommand;

#[async_trait]
impl SlashCommand for RewindCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/rewind",
            description: "open the rewind/checkpoint picker (Esc-Esc also opens this)",
            args_hint: "",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::Overlay(Overlay::Rewind))
    }
}

/// `/heapdump` — capture a heap profile, or tell the user to rebuild
/// with `--features=jemalloc-prof`.
pub(crate) struct HeapdumpCommand;

#[async_trait]
impl SlashCommand for HeapdumpCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/heapdump",
            description: "capture a heap profile (requires --features=jemalloc-prof)",
            args_hint: "",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/heapdump \u{2014} jemalloc-prof feature is not enabled in this build; rebuild caliban with `--features=jemalloc-prof` to capture profiles".into(),
        ))
    }
}

/// `/feedback` — open a markdown editor and submit to a configured
/// endpoint. No-op in OSS builds without `feedback_url`.
pub(crate) struct FeedbackCommand;

#[async_trait]
impl SlashCommand for FeedbackCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/feedback",
            description: "submit feedback to the configured endpoint",
            args_hint: "",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/feedback \u{2014} no endpoint configured. Set the `feedback_url` setting (Settings hierarchy spec) or file an issue on GitHub".into(),
        ))
    }
}

/// `/loop` — re-invoke the last assistant turn every N seconds until a
/// stop condition. Bounded by `--max-turns`.
pub(crate) struct LoopCommand;

#[async_trait]
impl SlashCommand for LoopCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/loop",
            description: "re-run the last assistant turn N times (bounded by --max-turns)",
            args_hint: "[--n=<count>] [--interval=<seconds>]",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let kv = super::parse_kv_args(args);
        let n = kv.get("n").and_then(|s| s.parse::<u32>().ok()).unwrap_or(3);
        let interval = kv
            .get("interval")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(15);
        let max_turns = ctx.app.args.max_turns;
        let bounded = std::cmp::min(n, max_turns);
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "/loop \u{2014} planned {n} repeats every {interval}s; bounded to {bounded} by --max-turns={max_turns} (execution lands with the polling scheduler spec)",
        )));
        Ok(SlashOutcome::Continue)
    }
}

/// `/statusline` — customize the status line via a shell template.
pub(crate) struct StatuslineCommand;

#[async_trait]
impl SlashCommand for StatuslineCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/statusline",
            description: "customize the status line via a shell-script template",
            args_hint: "",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let configured = ctx
            .app
            .settings_handle
            .as_ref()
            .and_then(|h| h.current().status_line.clone());
        let msg = match configured {
            Some(cfg) => format!(
                "/statusline \u{2014} active: `{}` (timeout {} ms, padding {}); refreshed after each turn",
                cfg.command, cfg.timeout_ms, cfg.padding,
            ),
            None => "/statusline \u{2014} unset; configure `statusLine.command` in settings.toml/.json to prefix a custom segment on the status bar".into(),
        };
        Ok(SlashOutcome::StatusMessage(msg))
    }
}

/// `/tui` — toggle fullscreen vs default TUI mode.
pub(crate) struct TuiCommand;

#[async_trait]
impl SlashCommand for TuiCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/tui",
            description: "toggle fullscreen vs default TUI mode",
            args_hint: "",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/tui \u{2014} alternate-screen toggle arrives with the TUI ergonomics spec".into(),
        ))
    }
}

/// `/voice` — voice dictation (hidden; reserved for future).
pub(crate) struct VoiceCommand;

#[async_trait]
impl SlashCommand for VoiceCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/voice",
            description: "voice dictation (reserved for future)",
            args_hint: "",
            hidden: true,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "voice dictation not available in this build".into(),
        ))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(RewindCommand));
    registry.register(Arc::new(HeapdumpCommand));
    registry.register(Arc::new(FeedbackCommand));
    registry.register(Arc::new(LoopCommand));
    registry.register(Arc::new(StatuslineCommand));
    registry.register(Arc::new(TuiCommand));
    registry.register(Arc::new(VoiceCommand));
}
