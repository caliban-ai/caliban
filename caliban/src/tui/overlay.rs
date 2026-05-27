//! Overlay enum and per-overlay rendering helpers.
//!
//! Overlays float above the main view (transcript + input). Each overlay
//! has its own line-builder that returns the contents to render inside the
//! bordered popup. The actual draw call is [`render_overlay`], invoked by
//! [`super::render::render`] when [`super::ViewState::Overlay`] is active.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::app::App;
use super::slash;
use super::transcript_viewer;

/// Top-level view state: normal main view or an open overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewState {
    Main,
    Overlay(Overlay),
}

/// Which overlay is currently being shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overlay {
    SlashHelp,
    Config,
    Mcp,
    #[allow(
        dead_code,
        reason = "Overlay::Skills retained for slash menu enum parity; /skills now prints to transcript"
    )]
    Skills,
    System,
    /// `Ctrl+O` transcript viewer overlay — drives `App.transcript_viewer`.
    TranscriptViewer,
    /// `Ctrl+R` reverse-history search overlay — drives `App.reverse_history`.
    ReverseHistory,
    /// Permission Ask modal — drives `App.ask_modal`.
    AskModal,
    /// `/rewind` overlay (ADR 0028). Lists per-prompt checkpoints with
    /// timestamps + file counts. Selectable actions: restore code /
    /// restore conversation / restore both / summarize from here /
    /// summarize up to here.
    Rewind,
}

impl Overlay {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::SlashHelp => "Slash Commands",
            Self::Config => "Configuration",
            Self::Mcp => "MCP Servers",
            Self::Skills => "Skills",
            Self::System => "System Prompt",
            Self::TranscriptViewer => "Transcript",
            Self::ReverseHistory => "Reverse History",
            Self::AskModal => "Permission Needed",
            Self::Rewind => "Rewind",
        }
    }

    pub(crate) fn short_name(self) -> &'static str {
        match self {
            Self::SlashHelp => "help",
            Self::Config => "config",
            Self::Mcp => "mcp",
            Self::Skills => "skills",
            Self::System => "system",
            Self::TranscriptViewer => "transcript",
            Self::ReverseHistory => "history",
            Self::AskModal => "ask",
            Self::Rewind => "rewind",
        }
    }
}

pub(crate) fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub(crate) fn render_overlay(frame: &mut ratatui::Frame<'_>, app: &App, overlay: Overlay) {
    use ratatui::widgets::Wrap;

    // The transcript viewer wants a tall overlay; the Ask modal stays compact.
    let (px, py) = match overlay {
        Overlay::AskModal => (60, 30),
        Overlay::ReverseHistory => (70, 50),
        _ => (80, 80),
    };
    let area = centered_rect(px, py, frame.area());

    // Clear the area underneath.
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", overlay.title()))
        .style(Style::default().fg(Color::White).bg(Color::Reset));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let scroll_offset = if overlay == Overlay::TranscriptViewer {
        app.transcript_viewer.scroll
    } else {
        0
    };

    let content_lines: Vec<Line<'static>> = match overlay {
        Overlay::SlashHelp => slash_help_lines(&app.slash_registry),
        Overlay::Config => clone_lines(&config_lines(app)),
        Overlay::Mcp => mcp_lines(app),
        Overlay::Skills => skills_lines(),
        Overlay::System => system_lines(app),
        Overlay::TranscriptViewer => {
            let mut lines =
                transcript_viewer::format_history(&app.messages, app.transcript_viewer.show_all);
            if app.transcript_viewer.show_help {
                lines.push(Line::raw(""));
                lines.extend(transcript_viewer::help_lines());
            }
            lines
        }
        Overlay::ReverseHistory => reverse_history_lines(app),
        Overlay::AskModal => ask_modal_lines(app),
        Overlay::Rewind => rewind_lines(app),
    };

    let body = Paragraph::new(content_lines)
        .scroll((scroll_offset, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

/// Clone `Line<'a>` to `Line<'static>` so the unified overlay renderer can
/// own its content. Most overlay-line builders already return `'static`
/// strings; `config_lines` is the lone holdout that borrows from `app`.
pub(crate) fn clone_lines(lines: &[Line<'_>]) -> Vec<Line<'static>> {
    lines
        .iter()
        .map(|l| {
            let spans: Vec<Span<'static>> = l
                .spans
                .iter()
                .map(|s| Span::styled(s.content.to_string(), s.style))
                .collect();
            let mut new_line = Line::from(spans);
            new_line.style = l.style;
            new_line.alignment = l.alignment;
            new_line
        })
        .collect()
}

pub(crate) fn reverse_history_lines(app: &App) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out: Vec<Line<'static>> = Vec::new();
    let Some(state) = app.reverse_history.as_ref() else {
        out.push(Line::raw("(no active search)"));
        return out;
    };
    out.push(Line::from(vec![
        Span::styled(
            format!(" scope: {}  ", state.scope.label()),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled("(Ctrl+S to cycle)", dim),
    ]));
    out.push(Line::from(vec![
        Span::raw(" query: "),
        Span::styled(state.query.clone(), Style::default().fg(Color::Cyan)),
    ]));
    out.push(Line::raw(""));
    let matches = state.matches();
    if matches.is_empty() {
        out.push(Line::styled("   (no matches)", dim));
    } else {
        for (i, m) in matches.iter().take(40).enumerate() {
            let style = if i == state.cursor {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            out.push(Line::from(Span::styled(format!("  {m}"), style)));
        }
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        " Enter: accept   Esc: cancel   Ctrl+S: cycle scope",
        dim,
    ));
    out
}

pub(crate) fn ask_modal_lines(app: &App) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut out: Vec<Line<'static>> = Vec::new();
    let Some(req) = app.ask_modal.as_ref() else {
        out.push(Line::raw("(no pending ask)"));
        return out;
    };
    out.push(Line::raw(""));
    out.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("Tool: ", bold),
        Span::raw(req.tool_name.clone()),
    ]));
    out.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("Input: ", bold),
        Span::raw(req.input_summary.clone()),
    ]));
    out.push(Line::raw(""));
    out.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("Pattern: ", bold),
        Span::raw(req.always_pattern.clone()),
    ]));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   [y] Allow once       [A] Always allow this pattern",
        Style::default().fg(Color::Cyan),
    ));
    out.push(Line::styled(
        "   [n] Reject once      [R] Always reject this pattern",
        Style::default().fg(Color::Cyan),
    ));
    out.push(Line::styled(
        "   [Esc] Deny",
        Style::default().fg(Color::Cyan),
    ));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   Modal blocks the agent loop until you decide.",
        dim,
    ));
    out
}

pub(crate) fn slash_help_lines(registry: &slash::SlashCommandRegistry) -> Vec<Line<'static>> {
    let entries: Vec<(String, String)> = registry
        .visible_metas()
        .into_iter()
        .map(|m| {
            let key = if m.args_hint.is_empty() {
                m.name.to_string()
            } else {
                format!("{} {}", m.name, m.args_hint)
            };
            (key, m.description.to_string())
        })
        .collect();

    let mut out = vec![Line::raw("")];
    for (cmd, desc) in entries {
        out.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("{cmd:<18}"), Style::default().fg(Color::Cyan)),
            Span::raw(desc),
        ]));
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        " \u{2014} Keyboard ",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    out.push(Line::raw(""));
    let keys = [
        ("Enter", "Submit prompt or slash command"),
        (
            "Shift+Enter / Alt+Enter",
            "Insert newline (multi-line input)",
        ),
        ("Backspace / Del", "Edit input"),
        ("Left / Right", "Move cursor"),
        ("Up / Down", "Navigate input history (or menu selection)"),
        ("Home / End", "Jump to start / end of input"),
        ("/", "Open slash-command menu"),
        (
            "@",
            "Open path-completion menu (auto-attaches file on submit)",
        ),
        ("Tab / Shift+Tab", "Cycle selection in an open menu"),
        ("PageUp / PageDn", "Scroll transcript"),
        (
            "Mouse wheel",
            "Scroll transcript (Opt/Shift+drag to select text)",
        ),
        ("Ctrl+C", "Cancel running turn or clear input"),
        ("Ctrl+D", "Exit (when input is empty)"),
        (
            "Esc / q",
            "Close menu or overlay (or cancel a running turn)",
        ),
    ];
    for (k, desc) in keys {
        out.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("{k:<18}"), Style::default().fg(Color::Cyan)),
            Span::raw(desc),
        ]));
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        "  Press q or Esc to close.",
        Style::default().add_modifier(Modifier::DIM),
    ));
    out
}

#[allow(clippy::too_many_lines)]
pub(crate) fn config_lines(app: &App) -> Vec<Line<'_>> {
    let provider = match app.args.provider {
        crate::ProviderKind::Anthropic => "anthropic",
        crate::ProviderKind::Openai => "openai",
        crate::ProviderKind::Ollama => "ollama",
        crate::ProviderKind::Google => "google",
    };
    let model = app
        .args
        .model
        .clone()
        .unwrap_or_else(|| crate::default_model_for(app.args.provider).to_string());

    let workspace = app
        .args
        .workspace
        .as_ref()
        .map_or_else(|| app.cwd_display(), |p| p.display().to_string());

    let tools_line = if app.args.no_tools {
        "disabled".to_string()
    } else {
        "enabled (Read, Write, Edit, Bash, Glob, Grep)".to_string()
    };

    let session_line = match &app.session {
        Some(s) => format!(
            "{} ({} turns, {} tokens)",
            s.name,
            s.turn_count(),
            s.total_usage
                .input_tokens
                .saturating_add(s.total_usage.output_tokens),
        ),
        None => "(ephemeral \u{2014} no session)".to_string(),
    };

    let sessions_dir = app.args.sessions_dir.as_ref().map_or_else(
        || match caliban_sessions::SessionStore::default_root() {
            Ok(p) => p.display().to_string(),
            Err(_) => "(unavailable)".to_string(),
        },
        |p| p.display().to_string(),
    );

    let temperature_line = match app.args.temperature {
        Some(t) => format!("{t}"),
        None => "(default)".to_string(),
    };

    let kv = |k: &'static str, v: String| -> Line<'static> {
        Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("{k:<20}"), Style::default().fg(Color::Cyan)),
            Span::raw(v),
        ])
    };

    let mut out = vec![Line::raw("")];
    out.push(kv("Provider", provider.to_string()));
    out.push(kv("Model", model));
    out.push(kv("Max tokens", app.args.max_tokens.to_string()));
    out.push(kv("Max turns", app.args.max_turns.to_string()));
    out.push(kv("Temperature", temperature_line));
    out.push(Line::raw(""));
    out.push(kv("Workspace root", workspace));
    out.push(kv("Restrict paths", app.args.restrict_paths.to_string()));
    out.push(kv("Tools", tools_line));
    out.push(Line::raw(""));
    out.push(kv("Sessions dir", sessions_dir));
    out.push(kv("Active session", session_line));
    out.push(Line::raw(""));
    out.push(kv("Quiet mode", app.args.quiet.to_string()));

    // Settings hierarchy section (ADR 0026). Lists the scope chain + a
    // few merged-effective values when the loader ran.
    if app.settings_handle.is_some() {
        out.push(Line::raw(""));
        out.push(Line::styled(
            "  Settings hierarchy",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        if app.settings_sources.is_empty() {
            out.push(Line::styled(
                "   (no scope files found — defaults in effect)",
                Style::default().add_modifier(Modifier::DIM),
            ));
        } else {
            for (label, path, format) in &app.settings_sources {
                let path_str = path
                    .as_ref()
                    .map_or_else(|| "(inline)".to_string(), |p| p.display().to_string());
                let fmt_str = format.as_deref().unwrap_or("?");
                out.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(format!("{label:<10}"), Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{path_str}  [{fmt_str}]")),
                ]));
            }
        }
        if let Some(handle) = app.settings_handle.as_ref() {
            let snap = handle.current();
            // Show three quick merged values + their formats.
            if let Some(m) = snap.model.as_ref() {
                out.push(kv("settings.model", m.display()));
            }
            if !snap.permissions.allow.is_empty() {
                out.push(kv(
                    "settings.allow",
                    format!("[{}]", snap.permissions.allow.join(", ")),
                ));
            }
            if !snap.permissions.deny.is_empty() {
                out.push(kv(
                    "settings.deny",
                    format!("[{}]", snap.permissions.deny.join(", ")),
                ));
            }
            if let Some(s) = snap.output_style.as_ref() {
                out.push(kv("settings.output_style", s.clone()));
            }
            if let Some(s) = snap.editor_mode.as_ref() {
                out.push(kv("settings.editor_mode", s.clone()));
            }
        }
    }

    out.push(Line::raw(""));
    out.push(Line::styled(
        "  Press q or Esc to close.",
        Style::default().add_modifier(Modifier::DIM),
    ));
    out
}

pub(crate) fn mcp_lines(app: &App) -> Vec<Line<'static>> {
    use caliban_mcp_client::ServerStatus;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![Line::raw("")];

    // Resolve the actual mcp.toml discovery paths so the help text and
    // the empty-state hint stay in sync with the loader's real behavior
    // (XDG-first with platform-native fallback; see
    // `caliban_mcp_client::config::discovery_paths`).
    let (user_candidates, project_path) = caliban_mcp_client::discovery_paths(app.cwd.as_path());
    let project_path_display = project_path.display().to_string();

    if app.mcp_servers.is_empty() {
        out.push(Line::raw("   No MCP servers configured."));
        out.push(Line::raw(""));
        out.push(Line::raw("   Configure servers at one of:"));
        for p in &user_candidates {
            out.push(Line::raw(format!("     {} (user)", p.display())));
        }
        out.push(Line::raw(format!("     {project_path_display} (project)")));
        out.push(Line::raw(""));
        out.push(Line::raw("   Minimal stdio example:"));
        out.push(Line::raw(""));
        out.push(Line::raw("     [server.silverbullet]"));
        out.push(Line::raw("     command = \"sb-mcp\""));
        out.push(Line::raw("     args = [\"--vault\", \"~/notes\"]"));
        out.push(Line::raw(""));
        out.push(Line::raw("   HTTP example:"));
        out.push(Line::raw(""));
        out.push(Line::raw("     [server.silverbullet]"));
        out.push(Line::raw("     type = \"http\""));
        out.push(Line::raw(
            "     url  = \"https://mcp.silverbullet.example/mcp\"",
        ));
        out.push(Line::raw(""));
        out.push(Line::styled(
            "   See `caliban-mcp-client` and ADR 0023 (transports + OAuth).",
            dim,
        ));
        out.push(Line::raw(""));
        out.push(Line::styled("  Press q or Esc to close.", dim));
        return out;
    }

    out.push(Line::raw("   MCP servers:"));
    out.push(Line::raw(""));

    for summary in &app.mcp_servers {
        let (glyph, glyph_style, status_text, status_style) = match &summary.status {
            ServerStatus::Connected { tools } => (
                "●",
                Style::default().fg(Color::Green),
                format!(
                    "connected — {tools} tool{}",
                    if *tools == 1 { "" } else { "s" }
                ),
                Style::default(),
            ),
            ServerStatus::Failed { reason } => (
                "○",
                Style::default().fg(Color::Red),
                format!("failed: {reason}"),
                Style::default().fg(Color::Red),
            ),
            ServerStatus::Disabled => ("○", dim, "disabled by mcp.toml".to_string(), dim),
        };
        let line = Line::from(vec![
            Span::raw("   "),
            Span::styled(glyph.to_string(), glyph_style),
            Span::raw(" "),
            Span::raw(format!("{:<12}", summary.name)),
            Span::raw(" "),
            Span::styled(
                format!("{:<6}", summary.transport),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(" "),
            Span::styled(status_text, status_style),
        ]);
        out.push(line);
    }

    out.push(Line::raw(""));
    out.push(Line::styled("   Config paths:", dim));
    for p in &user_candidates {
        out.push(Line::styled(format!("     {} (user)", p.display()), dim));
    }
    out.push(Line::styled(
        format!("     {project_path_display} (project)"),
        dim,
    ));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   Glyphs: \u{25CF} connected · \u{25D0} needs reauth · \u{25CB} disabled/failed",
        dim,
    ));
    out.push(Line::styled(
        "   [d] disable · [r] reload · [a] start OAuth · [s] view stderr · [t] tools",
        dim,
    ));
    out.push(Line::raw(""));
    out.push(Line::styled("  Press q or Esc to close.", dim));
    out
}

pub(crate) fn skills_lines() -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![Line::raw("")];
    out.push(Line::raw("   No skills configured."));
    out.push(Line::raw(""));
    out.push(Line::raw(
        "   Skills are reusable instruction-and-procedure packages the",
    ));
    out.push(Line::raw(
        "   model can invoke via a Skill tool. They mirror Claude",
    ));
    out.push(Line::raw("   Code's superpowers / skills design."));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   Planned configuration:",
        Style::default().fg(Color::Yellow),
    ));
    out.push(Line::raw("     ~/.config/caliban/skills/"));
    out.push(Line::raw("         <skill-name>/"));
    out.push(Line::raw("             SKILL.md         (instruction set)"));
    out.push(Line::raw(
        "             scripts/         (optional helper scripts)",
    ));
    out.push(Line::raw(
        "             references/      (optional reference docs)",
    ));
    out.push(Line::raw(""));
    out.push(Line::raw(
        "   The Skill tool would dispatch to the matching skill, load",
    ));
    out.push(Line::raw(
        "   its SKILL.md, and inject the content into the agent's",
    ));
    out.push(Line::raw("   context."));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   See caliban-skills (future sub-project) — not yet shipped.",
        dim,
    ));
    out.push(Line::raw(""));
    out.push(Line::styled("  Press q or Esc to close.", dim));
    out
}

pub(crate) fn system_lines(app: &App) -> Vec<Line<'static>> {
    let system_text = app
        .session
        .as_ref()
        .and_then(|s| {
            s.messages
                .iter()
                .find(|m| m.role == caliban_provider::Role::System)
        })
        .and_then(|m| {
            m.content.iter().find_map(|c| match c {
                caliban_provider::ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
        })
        .or_else(|| app.system_prompt.clone());

    let mut out = vec![Line::raw("")];
    match system_text {
        Some(text) => {
            for line in text.lines() {
                out.push(Line::raw(line.to_string()));
            }
        }
        None => {
            out.push(Line::raw(
                "(no system prompt — use --system or --system-file to set one)",
            ));
        }
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        "  Press q or Esc to close. Edit via --system-file or by editing the session JSON.",
        Style::default().add_modifier(Modifier::DIM),
    ));
    out
}

/// Render the `/rewind` overlay (ADR 0028) — listing per-prompt
/// checkpoints, newest first, with the actions available for the
/// currently-selected entry.
pub(crate) fn rewind_lines(app: &App) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = vec![Line::raw("")];
    let Some(store) = app.checkpoint_store.as_ref() else {
        out.push(Line::styled(
            "  (checkpointing not enabled for this session)",
            Style::default().add_modifier(Modifier::DIM),
        ));
        out.push(Line::raw(""));
        out.push(Line::raw(
            "  Checkpointing is opt-in. Disabled when CALIBAN_CHECKPOINT_DISABLED=1",
        ));
        out.push(Line::raw(
            "  is set, or when caliban was started without a checkpoint store wired in.",
        ));
        return out;
    };
    let prompts = match store.list_prompts() {
        Ok(p) => p,
        Err(e) => {
            out.push(Line::styled(
                format!("  error listing checkpoints: {e}"),
                Style::default().fg(Color::Red),
            ));
            return out;
        }
    };
    if prompts.is_empty() {
        out.push(Line::styled(
            "  (no checkpoints yet — send a prompt to create one)",
            Style::default().add_modifier(Modifier::DIM),
        ));
        return out;
    }
    for p in &prompts {
        let ts = p.created_at.format("%H:%M").to_string();
        let kind_tag = match p.kind {
            caliban_checkpoint::ManifestKind::Plan => "plan".to_string(),
            caliban_checkpoint::ManifestKind::Cleared => "cleared".to_string(),
            caliban_checkpoint::ManifestKind::Files => format!("{} file(s)", p.file_count),
        };
        let title = if p.title.is_empty() {
            "(no title)".to_string()
        } else {
            p.title.clone()
        };
        let prefix = if p.partial { "⚠ " } else { "   " };
        out.push(Line::from(vec![
            Span::raw(prefix.to_string()),
            Span::styled(
                format!("#{:>3}  ", p.prompt_index),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(format!("{title:<40} {ts}  {kind_tag}")),
        ]));
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        "  Actions: [c] code  [v] conversation  [b] both  [s] summarize→  [S] summarize←",
        Style::default().add_modifier(Modifier::DIM),
    ));
    out.push(Line::styled(
        "  ℹ Bash and external writes are NOT checkpointed.",
        Style::default().add_modifier(Modifier::DIM),
    ));
    out
}
