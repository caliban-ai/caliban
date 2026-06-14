//! Commands that already shipped before ADR 0040: `/plan`, `/memory`,
//! `/skills`, `/output-style`. The bodies move into trait impls here;
//! the behavior is preserved verbatim.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};
use crate::tui::TranscriptLine;

/// `/plan` — toggle plan mode (mutating tools blocked while ON).
pub(crate) struct PlanCommand;

#[async_trait]
impl SlashCommand for PlanCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/plan",
            description: "toggle plan mode (mutating tools blocked when on)",
            args_hint: "",
            hidden: false,
            immediate: false,
        }
    }
    async fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        use std::sync::atomic::Ordering;
        let now = !ctx.app.plan_mode.load(Ordering::Relaxed);
        ctx.app.plan_mode.store(now, Ordering::Relaxed);
        if let Some(sess) = ctx.app.session.as_mut() {
            sess.plan_mode = now;
        }
        let msg = if now {
            "plan mode: ON \u{2014} mutating tools blocked until /plan toggles off"
        } else {
            "plan mode: OFF \u{2014} mutating tools available"
        };
        ctx.app.transcript.push(TranscriptLine::Info(msg.into()));
        Ok(SlashOutcome::Continue)
    }
}

/// `/skills` — list loaded skills.
pub(crate) struct SkillsCommand;

#[async_trait]
impl SlashCommand for SkillsCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/skills",
            description: "list skills loaded from .caliban/skills/",
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
        let roots = caliban_skills::default_roots(&workspace_root);
        let skills = caliban_skills::load_skills(&roots);
        if skills.is_empty() {
            ctx.app.transcript.push(TranscriptLine::Info(
                "no skills loaded (drop a SKILL.md under .caliban/skills/<name>/)".into(),
            ));
        } else {
            ctx.app.transcript.push(TranscriptLine::Info(format!(
                "{} skill(s) loaded:",
                skills.len()
            )));
            for s in &skills {
                let first = s.description.lines().next().unwrap_or("");
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "  {} \u{2014} {}",
                    s.name, first
                )));
            }
        }
        Ok(SlashOutcome::Continue)
    }
}

/// `/memory` — three-tier memory summary or `list`/`show`/`edit`/`delete`.
pub(crate) struct MemoryCommand;

#[async_trait]
impl SlashCommand for MemoryCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/memory",
            description: "view or edit memory tiers and topic files",
            args_hint: "[list|show <slug>|edit <slug>|delete <slug>]",
            hidden: false,
            immediate: true,
        }
    }
    #[allow(clippy::too_many_lines)]
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let workspace_root = ctx
            .app
            .args
            .workspace
            .clone()
            .unwrap_or_else(|| ctx.app.cwd.clone());
        let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);

        let mut parts = args.splitn(2, char::is_whitespace);
        let sub = parts.next().unwrap_or("").trim();
        let rest = parts.next().unwrap_or("").trim();
        match sub {
            "" => match caliban_memory::load(&cfg).await {
                Ok(p) => {
                    ctx.app.transcript.push(TranscriptLine::Info(format!(
                        "memory tiers ({} tokens / {} budget):",
                        p.estimated_tokens, cfg.max_tokens
                    )));
                    for line in p.summary_lines() {
                        ctx.app.transcript.push(TranscriptLine::Info(line));
                    }
                    if p.truncated {
                        ctx.app.transcript.push(TranscriptLine::Info(
                            "(some tiers truncated \u{2014} raise CALIBAN_MEMORY_BUDGET_TOKENS or trim)"
                                .into(),
                        ));
                    }
                    ctx.app.transcript.push(TranscriptLine::Info(
                        "subcommands: /memory list | show <slug> | edit <slug> | delete <slug>"
                            .into(),
                    ));
                }
                Err(e) => {
                    ctx.app
                        .transcript
                        .push(TranscriptLine::Error(format!("memory load failed: {e}")));
                }
            },
            "list" => {
                let loader = caliban_memory::TopicLoader::new(cfg.auto_memory_dir.clone());
                match loader.list() {
                    Ok(topics) if topics.is_empty() => {
                        ctx.app
                            .transcript
                            .push(TranscriptLine::Info("no topic files yet".into()));
                    }
                    Ok(topics) => {
                        ctx.app.transcript.push(TranscriptLine::Info(format!(
                            "{} topic file(s) in {}:",
                            topics.len(),
                            cfg.auto_memory_dir.display()
                        )));
                        for t in topics {
                            ctx.app.transcript.push(TranscriptLine::Info(format!(
                                "  {} [{}] \u{2014} {}",
                                t.name,
                                t.kind.as_str(),
                                t.description
                            )));
                        }
                    }
                    Err(e) => {
                        ctx.app
                            .transcript
                            .push(TranscriptLine::Error(format!("list failed: {e}")));
                    }
                }
            }
            "show" => {
                if rest.is_empty() {
                    ctx.app
                        .transcript
                        .push(TranscriptLine::Info("usage: /memory show <slug>".into()));
                    return Ok(SlashOutcome::Continue);
                }
                let loader = caliban_memory::TopicLoader::new(cfg.auto_memory_dir.clone());
                match loader.read(rest) {
                    Ok(topic) => {
                        ctx.app.transcript.push(TranscriptLine::Info(format!(
                            "{} [{}] \u{2014} {}",
                            topic.name,
                            topic.kind.as_str(),
                            topic.description,
                        )));
                        for line in topic.body.lines() {
                            ctx.app
                                .transcript
                                .push(TranscriptLine::Info(line.to_string()));
                        }
                    }
                    Err(e) => {
                        ctx.app
                            .transcript
                            .push(TranscriptLine::Error(format!("show failed: {e}")));
                    }
                }
            }
            "edit" => {
                if rest.is_empty() {
                    ctx.app
                        .transcript
                        .push(TranscriptLine::Info("usage: /memory edit <slug>".into()));
                    return Ok(SlashOutcome::Continue);
                }
                let loader = caliban_memory::TopicLoader::new(cfg.auto_memory_dir.clone());
                if let Err(e) = caliban_memory::auto::validate_slug(rest) {
                    ctx.app
                        .transcript
                        .push(TranscriptLine::Error(format!("bad slug: {e}")));
                    return Ok(SlashOutcome::Continue);
                }
                let path = loader.dir().join(format!("{rest}.md"));
                if !path.exists() {
                    ctx.app.transcript.push(TranscriptLine::Error(format!(
                        "no such topic: {}",
                        path.display()
                    )));
                    return Ok(SlashOutcome::Continue);
                }
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_else(|_| "vi".to_string());
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "opening {} in {} (Ctrl-Z and `fg` to return, or run from outside the TUI)",
                    path.display(),
                    editor
                )));
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "\u{2192} run: {editor} {}",
                    path.display()
                )));
            }
            "delete" | "rm" => {
                if rest.is_empty() {
                    ctx.app.transcript.push(TranscriptLine::Info(
                        "usage: /memory delete <slug>  (also: /memory rm <slug>)".into(),
                    ));
                    return Ok(SlashOutcome::Continue);
                }
                let loader = caliban_memory::TopicLoader::new(cfg.auto_memory_dir.clone());
                match loader.delete(rest) {
                    Ok(()) => {
                        ctx.app
                            .transcript
                            .push(TranscriptLine::Info(format!("deleted topic '{rest}'")));
                    }
                    Err(e) => {
                        ctx.app
                            .transcript
                            .push(TranscriptLine::Error(format!("delete failed: {e}")));
                    }
                }
            }
            other => {
                ctx.app.transcript.push(TranscriptLine::Info(format!(
                    "unknown /memory subcommand: {other} \u{2014} try /memory list"
                )));
            }
        }
        Ok(SlashOutcome::Continue)
    }
}

/// `/output-style` — show active output style + the available list.
pub(crate) struct OutputStyleCommand;

#[async_trait]
impl SlashCommand for OutputStyleCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/output-style",
            description: "show the active output style and the available list",
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
        let reg = caliban_output_styles::OutputStylesRegistry::load(&workspace_root);
        let requested = caliban_output_styles::requested_from_env();
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "active output style: {requested} (set via {} env var; full UI ships with ADR 0040)",
            caliban_output_styles::ACTIVE_STYLE_ENV,
        )));
        ctx.app.transcript.push(TranscriptLine::Info(format!(
            "{} style(s) available:",
            reg.len()
        )));
        for s in reg.available() {
            let marker = if s.name == requested { "*" } else { " " };
            let badge = if s.force_for_plugin {
                " [force_for_plugin \u{2014} inert until ADR 0030]"
            } else {
                ""
            };
            ctx.app.transcript.push(TranscriptLine::Info(format!(
                "  {marker} {} \u{2014} {}{badge}",
                s.name, s.description
            )));
        }
        ctx.app.transcript.push(TranscriptLine::Info(
            "note: applies after /clear or restart (system prompts are cached)".into(),
        ));
        Ok(SlashOutcome::Continue)
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(PlanCommand));
    registry.register(Arc::new(SkillsCommand));
    registry.register(Arc::new(MemoryCommand));
    registry.register(Arc::new(OutputStyleCommand));
}
