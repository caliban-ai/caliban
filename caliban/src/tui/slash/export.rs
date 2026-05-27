//! `/export [path]` — write the session transcript to a markdown file.
//!
//! Clipboard support (path == `-`) and JSON format are wired here so the
//! shape stays close to the spec; arboard isn't pulled in as a dep, so
//! `-` emits a friendly error pointing at the path form. The default
//! path (no argument) is `caliban-session-<date>.md` in the CWD.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};

/// Output format for `/export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportFormat {
    Markdown,
    Json,
}

/// `/export` slash command.
pub(crate) struct ExportCommand;

#[async_trait]
impl SlashCommand for ExportCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/export",
            description: "export the session transcript to markdown (or json)",
            args_hint: "[path] [--format json]",
            hidden: false,
        }
    }

    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let format = if args.contains("--format json") {
            ExportFormat::Json
        } else {
            ExportFormat::Markdown
        };
        let body = render_session(ctx.app, format);
        let raw_path = args
            .split_whitespace()
            .find(|t| !t.starts_with("--"))
            .map(str::to_string);
        let path = match raw_path.as_deref() {
            Some("-") => {
                return Ok(SlashOutcome::StatusMessage(
                    "/export: clipboard support not wired in this build \u{2014} pass a path"
                        .into(),
                ));
            }
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let suffix = match format {
                    ExportFormat::Markdown => "md",
                    ExportFormat::Json => "json",
                };
                let default = format!("caliban-session-{}.{suffix}", Utc::now().format("%Y-%m-%d"));
                std::path::PathBuf::from(default)
            }
        };
        match std::fs::write(&path, body) {
            Ok(()) => Ok(SlashOutcome::StatusMessage(format!(
                "exported to {}",
                path.display()
            ))),
            Err(e) => Ok(SlashOutcome::StatusMessage(format!("/export: {e}"))),
        }
    }
}

/// Render the active session to a serialized text payload (Markdown or
/// JSON). Pulled out so the test can exercise it with a hand-rolled
/// `App`.
pub(crate) fn render_session(app: &crate::tui::app::App, format: ExportFormat) -> String {
    match format {
        ExportFormat::Markdown => render_markdown(app),
        ExportFormat::Json => render_json(app),
    }
}

fn render_markdown(app: &crate::tui::app::App) -> String {
    use caliban_provider::{ContentBlock, Role};
    use std::fmt::Write as _;
    let mut out = String::from("# caliban session ");
    out.push_str(&Utc::now().format("%Y-%m-%d").to_string());
    out.push_str("\n\n");
    let _ = writeln!(
        out,
        "- model: {}\n- messages: {}\n",
        app.agent.active_model().as_str(),
        app.messages.len(),
    );
    for (i, m) in app.messages.iter().enumerate() {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let _ = writeln!(out, "## Turn {} \u{2014} {role}\n", i + 1);
        for cb in &m.content {
            match cb {
                ContentBlock::Text(t) => {
                    out.push_str(&t.text);
                    out.push_str("\n\n");
                }
                ContentBlock::ToolUse(u) => {
                    let _ = writeln!(out, "### Tool: {}\n\n```json", u.name);
                    out.push_str(&u.input.to_string());
                    out.push_str("\n```\n\n");
                }
                ContentBlock::ToolResult(r) => {
                    out.push_str("```\n");
                    for inner in &r.content {
                        if let ContentBlock::Text(t) = inner {
                            out.push_str(&t.text);
                            out.push('\n');
                        }
                    }
                    out.push_str("```\n\n");
                }
                ContentBlock::Thinking(t) => {
                    out.push_str("### Thinking\n\n");
                    out.push_str(&t.thinking);
                    out.push_str("\n\n");
                }
                ContentBlock::Image(_) => {
                    out.push_str("_(image elided)_\n\n");
                }
            }
        }
    }
    out
}

fn render_json(app: &crate::tui::app::App) -> String {
    // Lean shape — caller-friendly subset of PersistedSession.
    let value = serde_json::json!({
        "model": app.agent.active_model().as_str(),
        "messages": app.messages,
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ExportCommand));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use caliban_provider::Message;
    use tempfile::tempdir;

    #[tokio::test]
    async fn export_writes_markdown_file() {
        let mut app = App::for_tests();
        app.messages.push(Message::user_text("hi"));
        app.messages.push(Message::assistant_text("hello"));
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.md");
        let mut ctx = app.slash_ctx_for_tests();
        let outcome = ExportCommand
            .execute(&path.to_string_lossy(), &mut ctx)
            .await
            .unwrap();
        match outcome {
            SlashOutcome::StatusMessage(s) => assert!(s.contains("exported")),
            other => panic!("unexpected outcome: {other:?}"),
        }
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("hi"));
        assert!(body.contains("hello"));
        assert!(body.starts_with("# caliban session"));
    }

    #[tokio::test]
    async fn export_writes_json_when_format_flag_set() {
        let mut app = App::for_tests();
        app.messages.push(Message::user_text("hi"));
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let arg = format!("{} --format json", path.to_string_lossy());
        let mut ctx = app.slash_ctx_for_tests();
        let _ = ExportCommand.execute(&arg, &mut ctx).await.unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(parsed["messages"].is_array());
    }
}
