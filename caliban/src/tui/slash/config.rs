//! Configuration / extensibility commands: `/config`, `/hooks`, `/mcp`,
//! `/plugins`, `/agents`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::{Overlay, TranscriptLine};

/// `/config` — open the tabbed settings editor overlay.
pub(crate) struct ConfigCommand;

#[async_trait]
impl SlashCommand for ConfigCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/config",
            description: "open the configuration overlay",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::Overlay(Overlay::Config))
    }
}

/// `/hooks` — render a one-line summary per configured event.
pub(crate) struct HooksCommand;

#[async_trait]
impl SlashCommand for HooksCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/hooks",
            description: "list configured hooks per event",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let workspace_root = ctx
            .app
            .args
            .workspace
            .clone()
            .unwrap_or_else(|| ctx.app.cwd.clone());
        // Legacy loader (deprecated) — kept here for the `/hooks` slash
        // overlay which surfaces the typed handler list from `hooks.toml`.
        // The canonical Settings-based summary lives in startup.rs.
        #[allow(deprecated)]
        let cfg = caliban_agent_core::HooksConfig::load(&workspace_root).unwrap_or_default();
        if cfg.total_handler_count() == 0 {
            ctx.app.transcript.push(TranscriptLine::Info(
                "no hooks configured (drop a hooks.toml under .caliban/ or your platform's user config dir for caliban)"
                    .into(),
            ));
        } else {
            ctx.app.transcript.push(TranscriptLine::Info(format!(
                "{} hook handler(s) loaded across {} event(s):",
                cfg.total_handler_count(),
                cfg.events.len()
            )));
            for (event, handlers) in &cfg.events {
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "  {event} \u{2192} {} handler(s)",
                    handlers.len()
                )));
            }
            if cfg.disable_all_hooks {
                ctx.app.transcript.push(TranscriptLine::Info(
                    "kill switch active (disable_all_hooks = true)".into(),
                ));
            }
        }
        Ok(SlashOutcome::Continue)
    }
}

/// `/mcp` — open the MCP server status overlay.
pub(crate) struct McpCommand;

#[async_trait]
impl SlashCommand for McpCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/mcp",
            description: "show MCP server status",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::Overlay(Overlay::Mcp))
    }
}

/// `/plugins` — list installed plugins with enable/disable status.
pub(crate) struct PluginsCommand {
    name: &'static str,
}

#[async_trait]
impl SlashCommand for PluginsCommand {
    fn meta(&self) -> &SlashCommandMeta {
        match self.name {
            "/plugin" => &SlashCommandMeta {
                name: "/plugin",
                description: "alias for /plugins",
                args_hint: "",
                hidden: true,
                immediate: true,
            },
            _ => &SlashCommandMeta {
                name: "/plugins",
                description: "list installed plugins",
                args_hint: "",
                hidden: false,
                immediate: true,
            },
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let workspace_root = ctx
            .app
            .args
            .workspace
            .clone()
            .unwrap_or_else(|| ctx.app.cwd.clone());
        let trust = caliban_plugins::TrustStore::open_default().unwrap_or_else(|_| {
            caliban_plugins::TrustStore {
                trust_path: std::path::PathBuf::new(),
                allowlist_path: std::path::PathBuf::new(),
                records: caliban_plugins::TrustFile::default(),
                allowlist: caliban_plugins::MarketplacesAllowlist::default(),
            }
        });
        let cli = caliban_plugins::Cli {
            workspace_root,
            user_install_dir: dirs::data_local_dir()
                .map(|d| d.join("caliban").join("plugins"))
                .unwrap_or_default(),
            trust,
            marketplace: caliban_plugins::MarketplaceClient::default(),
            settings: caliban_plugins::PluginSettings::from_env(),
        };
        match cli.list() {
            Ok(rows) => {
                for line in caliban_plugins::render_overlay(&rows) {
                    ctx.app.transcript.push(TranscriptLine::Info(line));
                }
            }
            Err(e) => {
                ctx.app
                    .transcript
                    .push(TranscriptLine::Error(format!("/plugins: {e}")));
            }
        }
        Ok(SlashOutcome::Continue)
    }
}

/// `/agents` — list configured sub-agents (ADR 0037).
pub(crate) struct AgentsCommand;

#[async_trait]
impl SlashCommand for AgentsCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/agents",
            description: "list sub-agents (full fleet overlay arrives with sub-agent isolation spec)",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        Ok(SlashOutcome::StatusMessage(
            "/agents \u{2014} full sub-agent fleet overlay arrives with the Sub-agent isolation spec (ADR 0037 follow-up); use `caliban agents list` from a shell for now".into(),
        ))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ConfigCommand));
    registry.register(Arc::new(HooksCommand));
    registry.register(Arc::new(McpCommand));
    registry.register(Arc::new(PluginsCommand { name: "/plugins" }));
    registry.register(Arc::new(PluginsCommand { name: "/plugin" }));
    registry.register(Arc::new(AgentsCommand));
}
