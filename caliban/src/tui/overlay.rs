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
    /// `/permissions` overlay. Shows the active permission mode +
    /// bypass-latch status + runtime rules added via the Ask modal's
    /// "Always allow/reject" branches. Interactive: `Tab` cycles mode
    /// (parity with `Shift+Tab` but discoverable in-overlay), `d`
    /// removes the selected runtime rule, arrows move the cursor.
    /// See `docs/superpowers/specs/2026-05-24-settings-hierarchy-design.md`.
    Permissions,
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
            Self::Permissions => "Permissions",
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
            Self::Permissions => "permissions",
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

/// Like [`centered_rect`] but with an absolute `width` and `height` in cells
/// (each clamped to `r`), centered both ways. Used for content-aware popups
/// like the Ask modal whose size tracks their contents instead of a flat
/// percentage, so neither the controls nor the body get clipped (#58).
pub(crate) fn centered_rect_abs(
    width: u16,
    height: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);
    ratatui::layout::Rect {
        x: r.x + r.width.saturating_sub(width) / 2,
        y: r.y + r.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Display width of a line in cells (sum of its spans' character counts).
fn line_width(line: &Line<'_>) -> u16 {
    u16::try_from(line.spans.iter().map(|s| s.content.chars().count()).sum::<usize>())
        .unwrap_or(u16::MAX)
}

/// Estimate how many terminal rows `lines` occupy when soft-wrapped to
/// `width` columns (ceil per line, minimum one row each). Approximates
/// ratatui's `Wrap` closely enough to size a popup so its contents are not
/// clipped.
fn wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    if width == 0 {
        return u16::try_from(lines.len()).unwrap_or(u16::MAX);
    }
    let w = usize::from(width);
    let mut rows: usize = 0;
    for line in lines {
        rows += usize::from(line_width(line)).div_ceil(w).max(1);
    }
    u16::try_from(rows).unwrap_or(u16::MAX)
}

pub(crate) fn render_overlay(frame: &mut ratatui::Frame<'_>, app: &App, overlay: Overlay) {
    use ratatui::widgets::Wrap;

    // Size the popup. The Ask modal is content-aware in BOTH dimensions: it
    // is made wide enough for its longest action row (so the Deny controls
    // never clip horizontally) and tall enough for the capped body plus the
    // action rows (so they never clip vertically) — clamped to the frame
    // (#58). Other overlays use a flat percentage.
    let area = match overlay {
        Overlay::AskModal => {
            // Right padding mirrors the 3-cell left indent baked into every
            // content line, so the box has symmetric inner margins.
            const PAD: u16 = 3;
            // Don't grow unboundedly wide for a long input — wrap past this.
            const MAX_INNER: u16 = 80;
            let body_lines = ask_modal_body_lines(app);
            let action_lines = ask_modal_action_lines(None);
            let widest = body_lines
                .iter()
                .chain(&action_lines)
                .map(line_width)
                .max()
                .unwrap_or(0)
                .min(MAX_INNER);
            let width = (widest + PAD + 2).min(frame.area().width);
            let avail = width.saturating_sub(2);
            let body_h = wrapped_height(&body_lines, avail).clamp(3, 12);
            let action_h = wrapped_height(&action_lines, avail);
            centered_rect_abs(width, body_h + action_h + 2, frame.area())
        }
        Overlay::ReverseHistory => centered_rect(70, 50, frame.area()),
        _ => centered_rect(80, 80, frame.area()),
    };

    // Clear the area underneath.
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", overlay.title()))
        .style(Style::default().fg(Color::White).bg(Color::Reset));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Ask modal: bottom-anchor the action rows so they are always visible.
    // The body (tool / input / pattern) takes the remaining space above and
    // wraps/clips there without ever hiding the controls (#58).
    if overlay == Overlay::AskModal {
        let action_lines = ask_modal_action_lines(Some(app.ask_cursor));
        // Reserve the actions' *wrapped* height so that on a terminal too
        // narrow for the two-column rows they wrap onto more lines instead of
        // being truncated at the right border. Both paragraphs wrap.
        let action_h = wrapped_height(&action_lines, inner.width).max(1);
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(action_h)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(ask_modal_body_lines(app)).wrap(Wrap { trim: false }),
            parts[0],
        );
        frame.render_widget(
            Paragraph::new(action_lines).wrap(Wrap { trim: false }),
            parts[1],
        );
        return;
    }

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
        Overlay::Permissions => permissions_lines(app),
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

/// Full Ask-modal content (body + action rows). Retained for tests and any
/// caller that wants the lines as one block; the live renderer instead draws
/// [`ask_modal_body_lines`] and [`ask_modal_action_lines`] into separate
/// regions so the controls can be bottom-anchored (#58).
pub(crate) fn ask_modal_lines(app: &App) -> Vec<Line<'static>> {
    let mut out = ask_modal_body_lines(app);
    out.extend(ask_modal_action_lines(None));
    out
}

/// Body of the Ask modal — tool name, input summary, and the pattern an
/// "Always" choice would persist. Separated from the action rows so the
/// renderer can keep the controls visible regardless of body length (#58).
pub(crate) fn ask_modal_body_lines(app: &App) -> Vec<Line<'static>> {
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
    out
}

/// Action rows + footer for the Ask modal. Always rendered (bottom-anchored
/// by the live renderer) so the controls can never be clipped by a long
/// body (#58). Laid out as a single-column "escalating" stack — allow once,
/// deny once, then the more-committal always-allow / always-deny, then Esc —
/// so the rows stay short and never wrap awkwardly side-by-side.
///
/// `selected` highlights one row (driven by the Up/Down cursor); pass `None`
/// for an unhighlighted block (sizing / the combined `ask_modal_lines`). The
/// selected/unselected prefixes are the same width so highlighting never
/// changes the modal's size.
pub(crate) fn ask_modal_action_lines(selected: Option<usize>) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let cyan = Style::default().fg(Color::Cyan);
    // Row order must match `events::ask_choice_for_cursor`.
    let labels = [
        "[y] Allow once",
        "[n] Deny once",
        "[a] Always allow (opens scope picker)",
        "[d] Always deny  (opens scope picker)",
        "[Esc] Deny once",
    ];
    let mut out = vec![Line::raw("")];
    for (i, label) in labels.iter().enumerate() {
        let is_sel = selected == Some(i);
        // 3-cell prefix either way so width is identical highlighted or not.
        let prefix = if is_sel { " \u{25b8} " } else { "   " };
        let style = if is_sel {
            cyan.add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            cyan
        };
        out.push(Line::styled(format!("{prefix}{label}"), style));
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   \u{2191}/\u{2193} move \u{00b7} Enter select \u{00b7} or press the [key].",
        dim,
    ));
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
    let provider = match crate::resolved_provider(&app.args) {
        crate::ProviderKind::Anthropic => "anthropic",
        crate::ProviderKind::Openai => "openai",
        crate::ProviderKind::Ollama => "ollama",
        crate::ProviderKind::Google => "google",
    };
    let model = app.args.model.clone().unwrap_or_else(|| {
        crate::default_model_for(crate::resolved_provider(&app.args)).to_string()
    });

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

/// `/permissions` overlay body.
///
/// Sections: tab header, current mode + bypass-latch status, then the
/// runtime-rule list (added via Ask modal "Always allow/reject" branches),
/// then key hints. The cursor (`app.permissions.cursor`) is rendered as `>`
/// on the selected runtime rule.
///
/// Pure with respect to `App` — only reads. Mutations (mode cycle,
/// rule remove, cursor move) happen in `events.rs::handle_key` under
/// the `ViewState::Overlay(Overlay::Permissions)` branch.
#[allow(clippy::too_many_lines)]
pub(crate) fn permissions_lines(app: &App) -> Vec<Line<'static>> {
    use crate::tui::app::PermissionsTab;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut out: Vec<Line<'static>> = Vec::new();

    // Tab header — render before the body so it's always visible at top.
    let tab_header = match app.permissions.tab {
        PermissionsTab::View => " View(▶)  Edit  Audit ",
        PermissionsTab::Edit => " View  Edit(▶)  Audit ",
        PermissionsTab::Audit => " View  Edit  Audit(▶) ",
    };
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(tab_header.to_string(), bold),
        Span::styled("  (Tab to switch tabs)", dim),
    ]));
    out.push(Line::raw(""));

    match app.permissions.tab {
        PermissionsTab::View | PermissionsTab::Edit => {
            // Mode line (shown on both View and Edit tabs).
            let mode = app.permission_mode.load();
            let chip = match mode {
                caliban_agent_core::PermissionMode::Default => "default".to_string(),
                other => other.chip().to_string(),
            };
            out.push(Line::raw(""));
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("Mode: ", bold),
                Span::styled(chip, Style::default().fg(Color::Yellow)),
                Span::raw("    "),
                Span::styled("(Tab to switch tabs / Shift+Tab cycle mode)", dim),
            ]));

            // Bypass-latch status.
            let latch_text = if app.bypass_latch {
                "armed (--allow-dangerously-skip-permissions)"
            } else {
                "not armed"
            };
            let latch_style = if app.bypass_latch {
                Style::default().fg(Color::Red)
            } else {
                dim
            };
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("Bypass latch: ", bold),
                Span::styled(latch_text.to_string(), latch_style),
            ]));

            out.push(Line::raw(""));
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("Effective rules (session > config files > built-in):", bold),
            ]));

            // Unified rule list: session + file-sourced + built-in defaults.
            let displayed = app.displayed_rules();
            if displayed.is_empty() {
                out.push(Line::styled("    (none)", dim));
            } else {
                use crate::tui::app::RuleOrigin;
                let cursor = app
                    .permissions
                    .cursor
                    .min(displayed.len().saturating_sub(1));
                for (i, r) in displayed.iter().enumerate() {
                    let prefix = if i == cursor { "  >" } else { "   " };
                    let action_label = match r.action {
                        caliban_agent_core::permissions::Action::Allow => "Allow ",
                        caliban_agent_core::permissions::Action::Deny => "Deny  ",
                        caliban_agent_core::permissions::Action::Ask => "Ask   ",
                    };
                    let action_style = match r.action {
                        caliban_agent_core::permissions::Action::Allow => {
                            Style::default().fg(Color::Green)
                        }
                        caliban_agent_core::permissions::Action::Deny => {
                            Style::default().fg(Color::Red)
                        }
                        caliban_agent_core::permissions::Action::Ask => {
                            Style::default().fg(Color::Yellow)
                        }
                    };
                    let scope_chip = match &r.origin {
                        RuleOrigin::Session => "[session]".to_string(),
                        RuleOrigin::File { scope, .. } => {
                            format!("[{}]", scope.label())
                        }
                        RuleOrigin::Default => "[default]".to_string(),
                    };
                    let chip_style = match &r.origin {
                        RuleOrigin::Session => Style::default().fg(Color::Cyan),
                        RuleOrigin::File { .. } => Style::default().fg(Color::Blue),
                        RuleOrigin::Default => dim,
                    };
                    let line_style = if i == cursor {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    out.push(
                        Line::from(vec![
                            Span::raw(format!("{prefix} [{i}] ")),
                            Span::styled(action_label.to_string(), action_style),
                            Span::raw(r.pattern.clone()),
                            Span::raw("  "),
                            Span::styled(scope_chip, chip_style),
                        ])
                        .style(line_style),
                    );
                }
            }

            out.push(Line::raw(""));
            if app.permissions.tab == PermissionsTab::Edit {
                // Edit-tab key hints.
                out.push(Line::styled(
                    "  Edit: [a] add rule  [d] delete rule  [p] promote to file  [t] test pane",
                    dim,
                ));
                out.push(Line::styled(
                    "  Nav:  \u{2191}\u{2193}/jk move  Esc/q close",
                    dim,
                ));
                out.push(Line::styled(
                    "  Tip: [d] deletes from the rule's source file; managed and defaults are read-only.",
                    dim,
                ));

                // Test pane inline output (when open).
                if let Some(tp) = app.permissions_test.as_ref() {
                    out.push(Line::raw(""));
                    out.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled("Test pane:", bold),
                        Span::styled("  Enter to run  Esc to close", dim),
                    ]));
                    out.push(Line::from(vec![
                        Span::raw("  tool: "),
                        Span::styled(
                            tp.tool_name.clone(),
                            if tp.focus == 0 {
                                Style::default().add_modifier(Modifier::REVERSED)
                            } else {
                                Style::default().fg(Color::Cyan)
                            },
                        ),
                    ]));
                    out.push(Line::from(vec![
                        Span::raw("  json: "),
                        Span::styled(
                            tp.input_json.clone(),
                            if tp.focus == 1 {
                                Style::default().add_modifier(Modifier::REVERSED)
                            } else {
                                Style::default().fg(Color::Cyan)
                            },
                        ),
                    ]));
                    if let Some(outcome) = tp.last_outcome.as_ref() {
                        out.push(Line::from(vec![
                            Span::raw("  result: "),
                            Span::styled(outcome.clone(), Style::default().fg(Color::Yellow)),
                        ]));
                    }
                }
            } else {
                // View-tab key hints.
                out.push(Line::styled(
                    "  Keys: Tab switch tab  Shift+Tab cycle mode  d delete rule  \u{2191}\u{2193}/jk move  Esc/q close",
                    dim,
                ));
                out.push(Line::styled(
                    "  Tip: [d] deletes from the rule's source file; managed and defaults are read-only.",
                    dim,
                ));
            }
        }
        PermissionsTab::Audit => {
            // Audit tab — the actual DecisionRecorder log lives at the path
            // returned by `caliban_agent_core::decision_log::decision_log_path()`.
            // For now, just point operators to `caliban perms audit` for the
            // richer viewer; the TUI viewer can be expanded later.
            out.push(Line::raw(""));
            let msg = if audit_log_exists() {
                "Audit log present. Run `caliban perms audit` for the full viewer."
            } else {
                "Audit log empty (enable with permissions.audit_log = true)."
            };
            out.push(Line::styled(msg.to_string(), dim));
            out.push(Line::raw(""));
            out.push(Line::styled("  Keys: Tab switch tab  Esc/q close", dim));
        }
    }

    out
}

/// Returns `true` if the canonical permissions decision log file exists.
/// Mirrors `caliban_agent_core::decision_log::decision_log_path` (without
/// importing it here to avoid a circular surface) and reads the same
/// `permission-decisions.jsonl` filename under
/// `$XDG_STATE_HOME/caliban/` (or the OS equivalent).
fn audit_log_exists() -> bool {
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("caliban")
        .join("permission-decisions.jsonl")
        .exists()
}

#[cfg(test)]
mod permissions_overlay_tests {
    use super::*;
    use crate::tui::App;
    use caliban_agent_core::PermissionMode;
    use caliban_agent_core::permissions::{Action, RuntimeRule};

    #[test]
    fn permissions_lines_shows_default_mode_and_no_rules_on_fresh_app() {
        let app = App::for_tests();
        let lines = permissions_lines(&app);
        let joined = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Mode: default"));
        // Default rules (e.g. Read, Bash) are always shown even on a fresh app.
        assert!(joined.contains("[default]"));
        assert!(joined.contains("Bypass latch: not armed"));
    }

    #[test]
    fn permissions_lines_renders_mode_chip_when_not_default() {
        let app = App::for_tests();
        app.permission_mode.store(PermissionMode::Plan);
        let lines = permissions_lines(&app);
        let joined = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("plan"));
        assert!(!joined.contains("Mode: default"));
    }

    #[test]
    fn permissions_lines_lists_runtime_rules_with_cursor() {
        let mut app = App::for_tests();
        app.runtime_rules.add(RuntimeRule {
            pattern: "Bash:ls *".into(),
            action: Action::Allow,
        });
        app.runtime_rules.add(RuntimeRule {
            pattern: "Edit(/tmp/*)".into(),
            action: Action::Deny,
        });
        app.permissions.cursor = 1;
        let lines = permissions_lines(&app);
        let joined = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("[0] Allow Bash:ls *"));
        assert!(joined.contains("[1] Deny  Edit(/tmp/*)"));
        // Cursor mark on row 1 (the selected one).
        let lines_text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let row1 = lines_text
            .iter()
            .find(|s| s.contains("[1]"))
            .expect("rule row 1");
        assert!(row1.starts_with("  >"));
    }

    #[test]
    fn permissions_lines_clamps_cursor_when_past_end() {
        let mut app = App::for_tests();
        app.runtime_rules.add(RuntimeRule {
            pattern: "a".into(),
            action: Action::Allow,
        });
        app.permissions.cursor = 99; // intentionally out of bounds
        // Should not panic; cursor clamps to the last row in the displayed
        // list (session rule + built-in defaults).
        let lines = permissions_lines(&app);
        let lines_text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // The session rule "a" appears at [0].
        assert!(
            lines_text.iter().any(|s| s.contains("[0]")),
            "rule row 0 must be present"
        );
        // At least one row must have the cursor mark (the last row in the list).
        assert!(
            lines_text.iter().any(|s| s.starts_with("  >")),
            "some row must have the cursor mark"
        );
    }

    /// Task 5.1 smoke test: all three tabs render without panic.
    #[test]
    fn permissions_overlay_renders_all_three_tabs() {
        use crate::tui::app::{PermissionsOverlayState, PermissionsTab};
        let mut app = App::for_tests();
        for tab in [
            PermissionsTab::View,
            PermissionsTab::Edit,
            PermissionsTab::Audit,
        ] {
            app.permissions = PermissionsOverlayState {
                tab,
                ..Default::default()
            };
            let lines = permissions_lines(&app);
            let joined: String = lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
                .collect::<String>();
            // Each tab should show the tab header marker.
            match tab {
                PermissionsTab::View => assert!(joined.contains("View(▶)"), "View tab header"),
                PermissionsTab::Edit => assert!(joined.contains("Edit(▶)"), "Edit tab header"),
                PermissionsTab::Audit => assert!(joined.contains("Audit(▶)"), "Audit tab header"),
            }
        }
    }

    /// Tab header shows the correct active-tab indicator.
    #[test]
    fn permissions_tab_header_reflects_active_tab() {
        use crate::tui::app::{PermissionsOverlayState, PermissionsTab};
        let mut app = App::for_tests();
        app.permissions = PermissionsOverlayState {
            tab: PermissionsTab::Edit,
            ..Default::default()
        };
        let lines = permissions_lines(&app);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains("Edit(▶)"), "Edit should be active");
        assert!(!joined.contains("View(▶)"), "View should not be active");
    }
}

#[cfg(test)]
mod ask_modal_render_tests {
    use super::*;
    use crate::tui::App;
    use crate::tui::ask::AskRequest;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Flatten the whole rendered backbuffer into one string for substring
    /// assertions. A clipped (off-screen) row is simply absent from the
    /// buffer, so `contains` is a faithful "is this visible?" check.
    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        term.backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    fn app_with_ask(input_summary: String, pattern: String) -> App {
        let mut app = App::for_tests();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        app.ask_modal = Some(AskRequest {
            tool_name: "Bash".into(),
            input_summary,
            always_pattern: pattern,
            tool_input: serde_json::json!({"command": "x"}),
            respond: tx,
        });
        app
    }

    /// Regression for #58: a long / overflowing input must not push the
    /// Deny action rows off the bottom of the Ask modal.
    #[test]
    fn ask_modal_keeps_deny_actions_visible_with_long_input() {
        let app = app_with_ask(
            format!("command={}", "x".repeat(2000)),
            format!("Bash:{}", "y".repeat(400)),
        );
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| render_overlay(f, &app, Overlay::AskModal))
            .unwrap();
        let text = buffer_text(&term);
        assert!(
            text.contains("[n]"),
            "Deny-once hint must stay visible with long input; rendered:\n{text}"
        );
        assert!(
            text.contains("[d]"),
            "Always-deny hint must stay visible with long input; rendered:\n{text}"
        );
    }

    /// The selected action row carries the cursor marker and others don't,
    /// and the marker never changes a row's width (so the modal can't resize
    /// as the cursor moves).
    #[test]
    fn ask_modal_action_lines_mark_only_the_selected_row() {
        let none = ask_modal_action_lines(None);
        let sel2 = ask_modal_action_lines(Some(2));
        let text = |l: &Line<'_>| -> String {
            l.spans.iter().map(|s| s.content.to_string()).collect()
        };
        // Row indices in the returned vec: [blank, r0, r1, r2, r3, r4, blank, footer].
        assert!(text(&sel2[3]).starts_with(" \u{25b8} "), "row 2 must be marked");
        assert!(
            text(&sel2[3]).contains("Always allow"),
            "row 2 is the always-allow row"
        );
        for i in [1usize, 2, 4, 5] {
            assert!(
                text(&sel2[i]).starts_with("   "),
                "non-selected row {i} keeps the plain indent"
            );
        }
        // Highlighting must not change widths.
        for (a, b) in none.iter().zip(sel2.iter()) {
            assert_eq!(line_width(a), line_width(b), "marker must preserve width");
        }
    }

    /// Regression for #58: on a narrower (e.g. half-screen) terminal the
    /// action labels must not be truncated at the right border (the modal was
    /// observed clipping "[a] Always allow (opens scope picker)" down to
    /// "[a] Always a"). The modal width is content-aware so the controls fit.
    #[test]
    fn ask_modal_keeps_action_labels_intact_on_narrow_terminal() {
        let app = app_with_ask(
            "command=find target -type f -executable".into(),
            "Bash:find target -type f -executable".into(),
        );
        // ~half-screen width that previously clipped the action rows.
        let mut term = Terminal::new(TestBackend::new(64, 24)).unwrap();
        term.draw(|f| render_overlay(f, &app, Overlay::AskModal))
            .unwrap();
        let text = buffer_text(&term);
        for needle in [
            "Always allow",
            "Always deny",
            "(opens scope picker)",
        ] {
            assert!(
                text.contains(needle),
                "action label {needle:?} must not be truncated on a narrow terminal; rendered:\n{text}"
            );
        }
    }
}
