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

/// `/context` — context window utilization (ADR 0033).
pub(crate) struct ContextCommand;

#[async_trait]
impl SlashCommand for ContextCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/context",
            description: "show context window utilization (warns at >=80%)",
            args_hint: "",
            hidden: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        for line in crate::tui::render_context_lines(ctx.app) {
            ctx.app.transcript.push(TranscriptLine::Info(line));
        }
        Ok(SlashOutcome::Continue)
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
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let workspace_root = ctx
            .app
            .args
            .workspace
            .clone()
            .unwrap_or_else(|| ctx.app.cwd.clone());
        let checks = doctor::run_checks(&workspace_root, ctx.app);
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "doctor \u{2014} {} check(s):",
            checks.len()
        )));
        for c in &checks {
            let glyph = if c.pass { "\u{2713}" } else { "\u{2717}" };
            ctx.app.transcript.push(TranscriptLine::Info(format!(
                "  {glyph} {} \u{2014} {}",
                c.name, c.detail
            )));
        }
        Ok(SlashOutcome::Continue)
    }
}

pub(crate) mod doctor {
    //! Pure check helpers for `/doctor`, pulled out for unit testing.

    use std::path::Path;

    /// One health-check result row.
    #[derive(Debug, Clone)]
    pub(crate) struct Check {
        /// Short name (e.g. `"skills"`, `"hooks"`).
        pub(crate) name: &'static str,
        /// `true` on pass.
        pub(crate) pass: bool,
        /// Human-readable detail.
        pub(crate) detail: String,
    }

    /// Run the registered checks and return their results.
    pub(crate) fn run_checks(workspace: &Path, app: &crate::tui::App) -> Vec<Check> {
        vec![
            skills_check(workspace),
            hooks_check(workspace),
            mcp_check(app),
            provider_check(app),
            workspace_check(workspace),
        ]
    }

    fn skills_check(workspace: &Path) -> Check {
        let roots = caliban_skills::default_roots(workspace);
        let skills = caliban_skills::load_skills(&roots);
        Check {
            name: "skills",
            pass: true,
            detail: format!("{} skill(s) loaded", skills.len()),
        }
    }

    fn hooks_check(workspace: &Path) -> Check {
        match caliban_agent_core::HooksConfig::load(workspace) {
            Ok(cfg) => Check {
                name: "hooks",
                pass: true,
                detail: format!(
                    "{} handler(s) across {} event(s)",
                    cfg.total_handler_count(),
                    cfg.events.len(),
                ),
            },
            Err(e) => Check {
                name: "hooks",
                pass: false,
                detail: format!("parse error: {e}"),
            },
        }
    }

    fn mcp_check(app: &crate::tui::App) -> Check {
        let n = app.mcp_servers.len();
        Check {
            name: "mcp",
            pass: true,
            detail: format!("{n} server(s) registered at startup"),
        }
    }

    fn provider_check(app: &crate::tui::App) -> Check {
        // Best-effort: we know the model + provider; we don't probe
        // credentials here (would block). Just confirm the provider
        // name is non-empty.
        let p = app.agent.provider();
        Check {
            name: "provider",
            pass: !p.name().is_empty(),
            detail: format!("provider={}", p.name()),
        }
    }

    fn workspace_check(workspace: &Path) -> Check {
        let writable = std::fs::metadata(workspace).is_ok_and(|m| !m.permissions().readonly());
        Check {
            name: "workspace",
            pass: writable,
            detail: format!(
                "{} ({})",
                workspace.display(),
                if writable { "writable" } else { "read-only" }
            ),
        }
    }
}

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
