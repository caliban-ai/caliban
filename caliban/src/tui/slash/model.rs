//! Model/auth commands: `/model`, `/effort`, `/status`, `/login`,
//! `/logout`, `/setup-token`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::TranscriptLine;

/// `/model` — show the current model and the router's per-purpose mapping.
pub(crate) struct ModelCommand;

#[async_trait]
impl SlashCommand for ModelCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/model",
            description: "show the active model and the router's per-purpose mapping",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let model = ctx
            .app
            .args
            .model
            .clone()
            .unwrap_or_else(|| crate::default_model_for(ctx.app.args.provider).to_string());
        let provider = ctx.app.agent.provider();
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "active model: {model} (provider: {})",
            provider.name(),
        )));
        ctx.app.transcript.push(TranscriptLine::Info(
            "per-purpose routing UI ships with the model router v2 spec".into(),
        ));
        Ok(SlashOutcome::Continue)
    }
}

/// `/effort` — cycle effort level (`low`/`medium`/`high`).
pub(crate) struct EffortCommand;

#[async_trait]
impl SlashCommand for EffortCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/effort",
            description: "cycle effort level (low/medium/high)",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/effort \u{2014} effort wiring lands with the model router v2 spec; takes effect on the next assistant turn once wired".into(),
        ))
    }
}

/// `/status` — auth status for each configured provider.
pub(crate) struct StatusCommand;

#[async_trait]
impl SlashCommand for StatusCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/status",
            description: "show provider/auth/subscription status",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let provider = ctx.app.agent.provider();
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "provider: {} (full provider/auth/subscription status arrives with the Auth spec)",
            provider.name(),
        )));
        Ok(SlashOutcome::Continue)
    }
}

/// `/login` — provider-specific auth flow stub.
pub(crate) struct LoginCommand;

#[async_trait]
impl SlashCommand for LoginCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/login",
            description: "run the active provider's auth flow",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/login \u{2014} provider-specific auth flow lands with the Auth spec (browser OAuth for Anthropic, `aws sso login` for Bedrock, `gcloud auth login` for Vertex)".into(),
        ))
    }
}

/// `/logout` — clear cached credentials stub.
pub(crate) struct LogoutCommand;

#[async_trait]
impl SlashCommand for LogoutCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/logout",
            description: "clear cached credentials for the active provider",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/logout \u{2014} credential clearing lands with the Auth spec; for now, unset provider env vars".into(),
        ))
    }
}

/// `/setup-token` — Anthropic long-lived OAuth token (CI use).
pub(crate) struct SetupTokenCommand;

#[async_trait]
impl SlashCommand for SetupTokenCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/setup-token",
            description: "generate a long-lived Anthropic OAuth token for CI",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/setup-token \u{2014} long-lived OAuth token issuance lands with the Auth spec".into(),
        ))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ModelCommand));
    registry.register(Arc::new(EffortCommand));
    registry.register(Arc::new(StatusCommand));
    registry.register(Arc::new(LoginCommand));
    registry.register(Arc::new(LogoutCommand));
    registry.register(Arc::new(SetupTokenCommand));
}
