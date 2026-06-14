//! Model/auth commands: `/model`, `/effort`, `/status`, `/login`,
//! `/logout`, `/setup-token`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::TranscriptLine;

/// `/model [id]` — switch the active model at runtime (same-provider
/// only in v1). With no arguments, lists the active provider's known
/// model ids and the currently-selected one to the transcript. Direct
/// `/model <id>` calls [`caliban_agent_core::Agent::try_swap_model`];
/// on success, the next turn picks up the new id, the context-window
/// capacity is re-seeded from the new model's `Capabilities`, and the
/// statusline reflects the new model immediately.
pub(crate) struct ModelCommand;

#[async_trait]
impl SlashCommand for ModelCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/model",
            description: "show or switch the active model (same-provider in v1)",
            args_hint: "[id]",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let trimmed = args.trim();
        let provider = ctx.app.agent.provider();
        if trimmed.is_empty() || trimmed == "--picker" {
            // Picker UI is deferred to a follow-up — surface the model
            // list inline so the operator can paste an id into `/model
            // <id>` to swap.
            let active = ctx.app.agent.active_model();
            ctx.app.transcript.push(TranscriptLine::Info(format!(
                "active model: {} (provider: {})",
                active.as_str(),
                provider.name(),
            )));
            let models = provider.list_models();
            if models.is_empty() {
                ctx.app.transcript.push(TranscriptLine::Info(
                    "no known-model list from this provider \u{2014} use `/model <id>` directly"
                        .into(),
                ));
            } else {
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "{} model(s) on `{}`:",
                    models.len(),
                    provider.name(),
                )));
                for m in &models {
                    let marker = if m.id.as_str() == active.as_str() {
                        "*"
                    } else {
                        " "
                    };
                    ctx.app.transcript.push(TranscriptLine::Info(format!(
                        "  {marker} {} \u{2014} {} ctx",
                        m.id, m.capabilities.max_input_tokens,
                    )));
                }
                ctx.app.transcript.push(TranscriptLine::Info(
                    "use `/model <id>` to switch. picker overlay lands in a follow-up.".into(),
                ));
            }
            return Ok(SlashOutcome::Continue);
        }
        match ctx.app.agent.try_swap_model(trimmed) {
            Ok(()) => {
                let caps = provider.capabilities(trimmed);
                ctx.app.context_window.set_capacity(caps.max_input_tokens);
                Ok(SlashOutcome::StatusMessage(format!(
                    "model \u{2192} {trimmed}"
                )))
            }
            Err(e) => Ok(SlashOutcome::StatusMessage(format!("/model: {e}"))),
        }
    }
}

/// `/effort <low|medium|high|max|auto>` — adjust reasoning effort at
/// runtime via the [`caliban_agent_core::Effort`] enum stored on the
/// Agent's `AgentConfig.effort` (`Arc<ArcSwap<Effort>>`). Takes effect on
/// the next assistant turn.
pub(crate) struct EffortCommand;

#[async_trait]
impl SlashCommand for EffortCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/effort",
            description: "set reasoning effort (low|medium|high|max|auto)",
            args_hint: "<level>",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        use caliban_agent_core::Effort;
        let level = match args.trim().to_ascii_lowercase().as_str() {
            "low" => Effort::Low,
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "max" => Effort::Max,
            "auto" => Effort::Auto,
            other => {
                return Ok(SlashOutcome::StatusMessage(format!(
                    "/effort: unknown level `{other}` (expected low|medium|high|max|auto)"
                )));
            }
        };
        ctx.app.agent.config().effort.store(Arc::new(level));
        Ok(SlashOutcome::StatusMessage(format!(
            "effort \u{2192} {level:?}"
        )))
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
            immediate: true,
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
            immediate: true,
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
            immediate: true,
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
            immediate: true,
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

#[cfg(test)]
mod model_command_tests {
    use super::*;
    use crate::tui::app::App;

    #[tokio::test]
    async fn model_with_id_swaps_active_model() {
        let mut app = App::for_tests_with_models(&["model-A", "model-B"]);
        assert_eq!(app.agent.active_model().as_str(), "model-A");
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ModelCommand.execute("model-B", &mut ctx).await.unwrap();
        match outcome {
            SlashOutcome::StatusMessage(s) => {
                assert!(s.contains("model-B"), "unexpected message: {s}");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(app.agent.active_model().as_str(), "model-B");
    }

    #[tokio::test]
    async fn model_with_unknown_id_reports_error_and_leaves_active_unchanged() {
        let mut app = App::for_tests_with_models(&["model-A"]);
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ModelCommand.execute("nope", &mut ctx).await.unwrap();
        match outcome {
            SlashOutcome::StatusMessage(s) => {
                assert!(
                    s.contains("not available") || s.contains("/model:"),
                    "unexpected message: {s}"
                );
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(app.agent.active_model().as_str(), "model-A");
    }

    #[tokio::test]
    async fn model_with_no_args_lists_known_models() {
        let mut app = App::for_tests_with_models(&["model-A", "model-B"]);
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ModelCommand.execute("", &mut ctx).await.unwrap();
        assert!(matches!(outcome, SlashOutcome::Continue));
        let info_lines: Vec<String> = app
            .transcript
            .iter()
            .filter_map(|l| match l {
                TranscriptLine::Info(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let body = info_lines.join("\n");
        assert!(body.contains("model-A"));
        assert!(body.contains("model-B"));
    }
}

#[cfg(test)]
mod effort_command_tests {
    use super::*;
    use crate::tui::app::App;
    use caliban_agent_core::Effort;

    #[tokio::test]
    async fn effort_low_updates_shared_state() {
        let mut app = App::for_tests();
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = EffortCommand.execute("low", &mut ctx).await.unwrap();
        assert!(matches!(outcome, SlashOutcome::StatusMessage(_)));
        assert_eq!(*ctx.app.agent.config().effort.load_full(), Effort::Low);
    }

    #[tokio::test]
    async fn effort_invalid_returns_error_message() {
        let mut app = App::for_tests();
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = EffortCommand.execute("turbo", &mut ctx).await.unwrap();
        match outcome {
            SlashOutcome::StatusMessage(s) => {
                assert!(
                    s.contains("expected low|medium|high|max|auto"),
                    "unexpected message: {s}"
                );
            }
            _ => panic!("unexpected outcome"),
        }
    }
}
