//! Ratatui-based interactive TUI.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::sync::Arc;

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
    Skills,
}

impl Overlay {
    fn title(self) -> &'static str {
        match self {
            Self::SlashHelp => "Slash Commands",
            Self::Config => "Configuration",
            Self::Mcp => "MCP Servers",
            Self::Skills => "Skills",
        }
    }

    fn short_name(self) -> &'static str {
        match self {
            Self::SlashHelp => "help",
            Self::Config => "config",
            Self::Mcp => "mcp",
            Self::Skills => "skills",
        }
    }
}

use anyhow::Result;
use caliban_agent_core::{Agent, TurnEventStream};
use caliban_sessions::{PersistedSession, SessionStore};
use crossterm::{
    event::{EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::Args;

/// RAII guard that enables raw mode + alternate screen on creation,
/// and restores the terminal on drop (including on panic).
pub(crate) struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    /// Enter raw mode and the alternate screen, constructing the guard.
    pub(crate) fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(out);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    /// Return a mutable reference to the underlying terminal.
    pub(crate) fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

/// A single entry in the scrolling transcript.
#[allow(dead_code)] // Variants used in T.2+ agent integration
#[derive(Debug, Clone)]
pub(crate) enum TranscriptLine {
    /// A prompt the user submitted.
    UserPrompt(String),
    /// A streamed assistant text chunk (accumulates in-place during T.2+).
    AssistantText(String),
    /// Streamed extended thinking text.
    AssistantThinking(String),
    /// A tool call (input may accumulate; result filled in on completion).
    ToolCall {
        /// Opaque tool-use identifier from the provider.
        tool_use_id: String,
        /// Tool name.
        name: String,
        /// Accumulated JSON input string.
        input: String,
        /// Optional `(is_error, result_text)` pair set when the call completes.
        result: Option<(bool, String)>,
    },
    /// Per-run token/turn summary appended at the end of a run.
    UsageSummary {
        /// Total input tokens consumed in the run.
        input_tokens: u32,
        /// Total output tokens generated in the run.
        output_tokens: u32,
        /// Number of agent turns taken.
        turn_count: u32,
    },
    /// Informational message (session save, slash-command output, …).
    Info(String),
    /// Error message.
    Error(String),
}

/// State for a currently-running agent turn.
#[allow(dead_code)] // Used in T.2+ agent integration
#[derive(Debug)]
pub(crate) struct RunningTurn {
    /// Cancel token — call `.cancel()` to interrupt the turn.
    pub(crate) cancel: tokio_util::sync::CancellationToken,
}

/// All TUI state: agent handle, session, display state, input buffer.
#[allow(dead_code)] // Fields used in T.2+ agent integration
pub(crate) struct App {
    /// The agent used to dispatch prompts.
    pub(crate) agent: Arc<Agent>,
    /// Active persisted session (if `--session` was given).
    pub(crate) session: Option<PersistedSession>,
    /// Session store for persistence.
    pub(crate) store: Option<SessionStore>,
    /// Parsed CLI arguments (provider, model, flags).
    pub(crate) args: Args,
    /// Current working directory (for the status bar).
    pub(crate) cwd: PathBuf,
    /// Resolved system prompt (None if --no-system was given).
    pub(crate) system_prompt: Option<String>,

    /// Scrolling output region contents.
    pub(crate) transcript: Vec<TranscriptLine>,
    /// The text currently in the input area.
    pub(crate) input: String,
    /// Byte offset of the cursor within `input`.
    pub(crate) cursor: usize,
    /// History of submitted prompts (oldest first).
    pub(crate) history: Vec<String>,
    /// Index into `history` while navigating; `None` when at the live input.
    pub(crate) history_index: Option<usize>,
    /// Manual scroll offset (rows from top) when `auto_scroll` is false.
    pub(crate) scroll: u16,
    /// When `true`, keep the viewport pinned to the bottom of the transcript.
    pub(crate) auto_scroll: bool,
    /// Set to `true` to break the event loop cleanly.
    pub(crate) should_exit: bool,
    /// Non-`None` while an agent turn is in progress.
    pub(crate) running: Option<RunningTurn>,
    /// Current view state: main view or an open overlay.
    pub(crate) view: ViewState,
}

impl App {
    /// Construct initial `App` state from CLI args and an optional loaded session.
    pub(crate) fn new(
        agent: Arc<Agent>,
        session: Option<PersistedSession>,
        store: Option<SessionStore>,
        args: Args,
        system_prompt: Option<String>,
    ) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let history = session
            .as_ref()
            .map(|s| {
                s.messages
                    .iter()
                    .filter_map(|m| {
                        if m.role == caliban_provider::Role::User {
                            Some(
                                m.content
                                    .iter()
                                    .filter_map(|cb| match cb {
                                        caliban_provider::ContentBlock::Text(t) => {
                                            Some(t.text.clone())
                                        }
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            )
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            agent,
            session,
            store,
            args,
            cwd,
            system_prompt,
            transcript: Vec::new(),
            input: String::new(),
            cursor: 0,
            history,
            history_index: None,
            scroll: 0,
            auto_scroll: true,
            should_exit: false,
            running: None,
            view: ViewState::Main,
        }
    }

    /// Insert a character at the current cursor position.
    pub(crate) fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the character immediately before the cursor.
    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.input[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, char::len_utf8);
        self.cursor -= prev;
        self.input.drain(self.cursor..self.cursor + prev);
    }

    /// Delete the character immediately after the cursor.
    pub(crate) fn delete_forward(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let next = self.input[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.input.drain(self.cursor..self.cursor + next);
    }

    /// Move the cursor one character to the left.
    pub(crate) fn cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.input[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, char::len_utf8);
        self.cursor -= prev;
    }

    /// Move the cursor one character to the right.
    pub(crate) fn cursor_right(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let next = self.input[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.cursor += next;
    }

    /// Move the cursor to the beginning of the input.
    pub(crate) fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Move the cursor to the end of the input.
    pub(crate) fn cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Navigate to the previous history entry.
    pub(crate) fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let new_idx = match self.history_index {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(n) => n - 1,
        };
        self.history_index = Some(new_idx);
        self.input = self.history[new_idx].clone();
        self.cursor = self.input.len();
    }

    /// Navigate to the next history entry (or clear the input when past the end).
    pub(crate) fn history_down(&mut self) {
        let Some(idx) = self.history_index else {
            return;
        };
        if idx + 1 >= self.history.len() {
            self.history_index = None;
            self.input.clear();
            self.cursor = 0;
        } else {
            self.history_index = Some(idx + 1);
            self.input = self.history[idx + 1].clone();
            self.cursor = self.input.len();
        }
    }

    /// Return the current working directory as a tilde-collapsed display string.
    pub(crate) fn cwd_display(&self) -> String {
        if let Some(home) = dirs::home_dir() {
            if let Ok(stripped) = self.cwd.strip_prefix(&home) {
                if stripped.as_os_str().is_empty() {
                    return "~".into();
                }
                return format!("~/{}", stripped.display());
            }
        }
        self.cwd.display().to_string()
    }
}

// === Rendering ===

#[allow(clippy::too_many_lines)]
fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // 0: output region (flex)
            Constraint::Length(1), // 1: top border (horizontal rule)
            Constraint::Length(1), // 2: input area
            Constraint::Length(1), // 3: bottom border
            Constraint::Length(1), // 4: status bar
        ])
        .split(frame.area());

    // chunks[0] = output region
    let lines = render_transcript(app);
    let total_lines = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let visible = chunks[0].height;
    let scroll = if app.auto_scroll {
        total_lines.saturating_sub(visible)
    } else {
        app.scroll
    };
    let output = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(output, chunks[0]);

    // chunks[1] = top horizontal rule
    let hrule_top = Block::default()
        .borders(Borders::TOP)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hrule_top, chunks[1]);

    // chunks[2] = input
    let input_line = Line::from(vec![Span::raw("> "), Span::raw(app.input.as_str())]);
    frame.render_widget(Paragraph::new(input_line), chunks[2]);

    // chunks[3] = bottom horizontal rule
    let hrule_bot = Block::default()
        .borders(Borders::TOP)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hrule_bot, chunks[3]);

    // chunks[4] = status bar
    let status = render_status(app);
    frame.render_widget(Paragraph::new(status), chunks[4]);

    // Cursor position — in chunks[2] (input area)
    let prefix_cols: u16 = u16::try_from(app.input[..app.cursor].chars().count()).unwrap_or(0);
    frame.set_cursor_position((chunks[2].x + 2 + prefix_cols, chunks[2].y));

    // Render overlay on top if one is active.
    if let ViewState::Overlay(o) = app.view {
        render_overlay(frame, app, o);
    }
}

#[allow(clippy::too_many_lines)]
fn render_transcript(app: &App) -> Vec<Line<'_>> {
    let mut lines: Vec<Line<'_>> = Vec::new();
    for entry in &app.transcript {
        match entry {
            TranscriptLine::UserPrompt(s) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "user: ",
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .fg(Color::Cyan),
                    ),
                    Span::raw(s.as_str()),
                ]));
                lines.push(Line::raw(""));
            }
            TranscriptLine::AssistantText(s) => {
                for line in s.split('\n') {
                    lines.push(Line::raw(line));
                }
            }
            TranscriptLine::AssistantThinking(s) => {
                lines.push(Line::styled(
                    format!("(thinking) {s}"),
                    Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
                ));
            }
            TranscriptLine::ToolCall {
                name,
                input,
                result,
                ..
            } => {
                let input_summary: String = input.chars().take(80).collect();
                lines.push(Line::styled(
                    format!("\u{1F527} {name}({input_summary})"),
                    Style::default().fg(Color::Yellow),
                ));
                if let Some((is_error, result_text)) = result {
                    let prefix = if *is_error { "(error) " } else { "" };
                    let summary: String = result_text.chars().take(80).collect();
                    lines.push(Line::styled(
                        format!("   \u{2192} {prefix}{summary}"),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
                lines.push(Line::raw(""));
            }
            TranscriptLine::UsageSummary {
                input_tokens,
                output_tokens,
                turn_count,
            } => {
                lines.push(Line::styled(
                    format!(
                        "[caliban: {turn_count} turns \u{00B7} \
                         {input_tokens}\u{2191} {output_tokens}\u{2193} tokens]"
                    ),
                    Style::default().add_modifier(Modifier::DIM),
                ));
                lines.push(Line::raw(""));
            }
            TranscriptLine::Info(s) => {
                lines.push(Line::styled(
                    format!("[{s}]"),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
            TranscriptLine::Error(s) => {
                lines.push(Line::styled(
                    format!("error: {s}"),
                    Style::default().fg(Color::Red),
                ));
            }
        }
    }
    lines
}

// === Overlay helpers ===

fn centered_rect(
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

fn render_overlay(frame: &mut ratatui::Frame<'_>, app: &App, overlay: Overlay) {
    use ratatui::widgets::Clear;
    use ratatui::widgets::Wrap;

    let area = centered_rect(80, 80, frame.area());

    // Clear the area underneath.
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", overlay.title()))
        .style(Style::default().fg(Color::White).bg(Color::Reset));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let content_lines = match overlay {
        Overlay::SlashHelp => slash_help_lines(),
        Overlay::Config => config_lines(app),
        Overlay::Mcp => mcp_lines(),
        Overlay::Skills => skills_lines(),
    };

    let body = Paragraph::new(content_lines).wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

fn slash_help_lines() -> Vec<Line<'static>> {
    let entries = [
        ("/help", "Show this help"),
        ("/exit, /quit", "Save session and exit"),
        ("/clear", "Clear transcript"),
        ("/sessions", "List saved sessions"),
        ("/save [<name>]", "Save current session (optionally rename)"),
        ("/usage", "Show accumulated usage"),
        ("/config", "Show active configuration"),
        ("/mcp", "MCP server configuration (stub)"),
        ("/skills", "Skills configuration (stub)"),
    ];

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
        ("Backspace / Del", "Edit input"),
        ("Left / Right", "Move cursor"),
        ("Up / Down", "Navigate input history"),
        ("Home / End", "Jump to start / end of input"),
        ("PageUp / PageDn", "Scroll transcript"),
        ("Ctrl+C", "Cancel running turn or clear input"),
        ("Ctrl+D", "Exit (when input is empty)"),
        ("Esc / q", "Close this overlay (or cancel a running turn)"),
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
fn config_lines(app: &App) -> Vec<Line<'_>> {
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
    out.push(Line::raw(""));
    out.push(Line::styled(
        "  Press q or Esc to close.",
        Style::default().add_modifier(Modifier::DIM),
    ));
    out
}

fn mcp_lines() -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![Line::raw("")];
    out.push(Line::raw("   No MCP servers configured."));
    out.push(Line::raw(""));
    out.push(Line::raw(
        "   MCP (Model Context Protocol) lets caliban consume external",
    ));
    out.push(Line::raw(
        "   tool servers — for example, a SilverBullet notebook, a",
    ));
    out.push(Line::raw(
        "   Linear ticket browser, or a custom in-house server.",
    ));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   Planned configuration:",
        Style::default().fg(Color::Yellow),
    ));
    out.push(Line::raw("     ~/.config/caliban/mcp.toml"));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   Example (future):",
        Style::default().fg(Color::Yellow),
    ));
    out.push(Line::raw("     [[server]]"));
    out.push(Line::raw("     name = \"silverbullet\""));
    out.push(Line::raw("     transport = \"stdio\""));
    out.push(Line::raw("     command = \"sb-mcp\""));
    out.push(Line::raw("     args = [\"--vault\", \"~/notes\"]"));
    out.push(Line::raw(""));
    out.push(Line::raw("     [[server]]"));
    out.push(Line::raw("     name = \"linear\""));
    out.push(Line::raw("     transport = \"http\""));
    out.push(Line::raw("     url = \"https://mcp.example.com/linear\""));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   See caliban-mcp-client (Layer 2 sub-project) — not yet shipped.",
        dim,
    ));
    out.push(Line::raw(""));
    out.push(Line::styled("  Press q or Esc to close.", dim));
    out
}

fn skills_lines() -> Vec<Line<'static>> {
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

fn render_status(app: &App) -> Line<'static> {
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

    let cwd = app.cwd_display();
    let session_part = if let Some(sess) = &app.session {
        format!(" \u{00B7} session: {} ({}t)", sess.name, sess.turn_count())
    } else {
        String::new()
    };
    let running_part = if app.running.is_some() {
        " \u{00B7} running\u{2026}".to_string()
    } else {
        String::new()
    };
    let overlay_part = match app.view {
        ViewState::Overlay(o) => format!(" \u{00B7} [{} \u{2014} q to close]", o.short_name()),
        ViewState::Main => String::new(),
    };

    let text =
        format!(" {cwd} \u{00B7} {provider} {model}{session_part}{overlay_part}{running_part}");
    Line::from(Span::styled(
        text,
        Style::default().bg(Color::DarkGray).fg(Color::White),
    ))
}

// === Agent event handlers ===

#[allow(clippy::too_many_lines)]
fn handle_agent_event(evt: caliban_agent_core::TurnEvent, app: &mut App) {
    use caliban_agent_core::TurnEvent;
    match evt {
        TurnEvent::TurnStart { .. } => {
            // No transcript change; render shows "running…" via app.running.
        }
        TurnEvent::AssistantTextDelta { text, .. } => {
            // Find or create the in-progress AssistantText line.
            if let Some(TranscriptLine::AssistantText(buf)) = app.transcript.last_mut() {
                buf.push_str(&text);
            } else {
                app.transcript.push(TranscriptLine::AssistantText(text));
            }
        }
        TurnEvent::AssistantThinkingDelta { text, .. } => {
            if let Some(TranscriptLine::AssistantThinking(buf)) = app.transcript.last_mut() {
                buf.push_str(&text);
            } else {
                app.transcript.push(TranscriptLine::AssistantThinking(text));
            }
        }
        TurnEvent::ToolCallStart {
            tool_use_id, name, ..
        } => {
            app.transcript.push(TranscriptLine::ToolCall {
                tool_use_id,
                name,
                input: String::new(),
                result: None,
            });
        }
        TurnEvent::ToolCallInputDelta {
            tool_use_id,
            partial_json,
            ..
        } => {
            for entry in app.transcript.iter_mut().rev() {
                if let TranscriptLine::ToolCall {
                    tool_use_id: id,
                    input,
                    ..
                } = entry
                {
                    if *id == tool_use_id {
                        input.push_str(&partial_json);
                        break;
                    }
                }
            }
        }
        TurnEvent::ToolCallEnd {
            tool_use_id,
            is_error,
            content,
            ..
        } => {
            let result_text = content
                .iter()
                .filter_map(|c| match c {
                    caliban_provider::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            for entry in app.transcript.iter_mut().rev() {
                if let TranscriptLine::ToolCall {
                    tool_use_id: id,
                    result,
                    ..
                } = entry
                {
                    if *id == tool_use_id {
                        *result = Some((is_error, result_text));
                        break;
                    }
                }
            }
        }
        TurnEvent::TurnEnd { .. } => {
            // Push a blank line for visual separation.
            app.transcript
                .push(TranscriptLine::AssistantText(String::new()));
        }
        TurnEvent::RunEnd {
            final_messages,
            total_usage,
            turn_count,
            ..
        } => {
            app.transcript.push(TranscriptLine::UsageSummary {
                input_tokens: total_usage.input_tokens,
                output_tokens: total_usage.output_tokens,
                turn_count,
            });
            // Update session and persist.
            if let Some(sess) = app.session.as_mut() {
                sess.merge_run(final_messages, total_usage);
                if let Some(store) = app.store.as_ref() {
                    if !app.args.no_save {
                        match store.save(sess) {
                            Ok(()) => app
                                .transcript
                                .push(TranscriptLine::Info("session saved".into())),
                            Err(e) => app
                                .transcript
                                .push(TranscriptLine::Error(format!("save failed: {e}"))),
                        }
                    }
                }
            }
            app.running = None;
            app.auto_scroll = true;
        }
    }
}

fn handle_agent_error(e: &caliban_agent_core::Error, app: &mut App) {
    if matches!(e, caliban_agent_core::Error::Cancelled) {
        app.transcript
            .push(TranscriptLine::Info("turn cancelled".into()));
    } else {
        app.transcript.push(TranscriptLine::Error(e.to_string()));
    }
    app.running = None;
}

// === Event loop ===

/// Run the TUI until the user exits (Ctrl+D or Ctrl+C with empty input).
///
/// Saves the session on clean exit if `--no-save` was not set.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run(
    args: Args,
    agent: Arc<Agent>,
    store: Option<SessionStore>,
    session: Option<PersistedSession>,
    system_prompt: Option<String>,
) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new(agent, session, store, args, system_prompt);
    let mut events = EventStream::new();
    let mut agent_stream: Option<TurnEventStream> = None;

    loop {
        guard.terminal().draw(|frame| render(frame, &app))?;
        if app.should_exit {
            break;
        }

        tokio::select! {
            term_event = events.next() => {
                let Some(Ok(ref ev)) = term_event else { break };
                handle_event(ev, &mut app, &mut agent_stream);
            }
            agent_event = async {
                if let Some(s) = agent_stream.as_mut() {
                    s.next().await
                } else {
                    std::future::pending::<Option<Result<caliban_agent_core::TurnEvent, caliban_agent_core::Error>>>().await
                }
            } => {
                match agent_event {
                    Some(Ok(evt)) => handle_agent_event(evt, &mut app),
                    Some(Err(ref e)) => {
                        handle_agent_error(e, &mut app);
                        agent_stream = None;
                    }
                    None => {
                        // Stream finished cleanly — running was already cleared by RunEnd.
                        app.running = None;
                        agent_stream = None;
                    }
                }
            }
        }
    }

    // Save session on clean exit (no-op if RunEnd already saved it).
    if let (Some(store), Some(sess)) = (app.store.as_ref(), app.session.as_ref()) {
        if !app.args.no_save {
            let _ = store.save(sess);
        }
    }

    Ok(())
}

fn handle_slash_command(line: &str, app: &mut App) {
    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match cmd {
        "/help" => {
            app.view = ViewState::Overlay(Overlay::SlashHelp);
        }
        "/config" => {
            app.view = ViewState::Overlay(Overlay::Config);
        }
        "/mcp" => {
            app.view = ViewState::Overlay(Overlay::Mcp);
        }
        "/skills" => {
            app.view = ViewState::Overlay(Overlay::Skills);
        }
        "/exit" | "/quit" => {
            app.should_exit = true;
        }
        "/clear" => {
            app.transcript.clear();
        }
        "/sessions" => match &app.store {
            Some(store) => match store.list() {
                Ok(list) if list.is_empty() => {
                    app.transcript
                        .push(TranscriptLine::Info("no sessions yet".into()));
                }
                Ok(list) => {
                    for m in list {
                        app.transcript.push(TranscriptLine::Info(format!(
                            "{} \u{2014} {} turns, {} tokens \u{2014} {}",
                            m.name,
                            m.turn_count,
                            m.total_tokens,
                            m.updated_at.format("%Y-%m-%d %H:%M:%S"),
                        )));
                    }
                }
                Err(e) => {
                    app.transcript
                        .push(TranscriptLine::Error(format!("list error: {e}")));
                }
            },
            None => {
                app.transcript
                    .push(TranscriptLine::Info("no session store".into()));
            }
        },
        "/save" => {
            if let (Some(store), Some(sess)) = (&app.store, app.session.as_ref()) {
                let target_name = if arg.is_empty() {
                    sess.name.clone()
                } else {
                    arg.to_string()
                };
                let mut to_save = sess.clone();
                to_save.name.clone_from(&target_name);
                match store.save(&to_save) {
                    Ok(()) => app
                        .transcript
                        .push(TranscriptLine::Info(format!("saved as '{target_name}'"))),
                    Err(e) => app
                        .transcript
                        .push(TranscriptLine::Error(format!("save error: {e}"))),
                }
            } else {
                app.transcript
                    .push(TranscriptLine::Info("no session to save".into()));
            }
        }
        "/usage" => match app.session.as_ref() {
            Some(s) => app.transcript.push(TranscriptLine::Info(format!(
                "session {}: {} turns, {} input + {} output tokens",
                s.name,
                s.turn_count(),
                s.total_usage.input_tokens,
                s.total_usage.output_tokens,
            ))),
            None => app
                .transcript
                .push(TranscriptLine::Info("no session active".into())),
        },
        unknown => {
            app.transcript.push(TranscriptLine::Info(format!(
                "unknown command: {unknown} \u{2014} type /help"
            )));
        }
    }
}

fn handle_event(
    event: &crossterm::event::Event,
    app: &mut App,
    agent_stream: &mut Option<TurnEventStream>,
) {
    use crossterm::event::Event;
    if let Event::Key(key) = event {
        if key.kind != KeyEventKind::Press {
            return;
        }
        handle_key(*key, app, agent_stream);
    }
}

#[allow(clippy::too_many_lines)]
fn handle_key(key: KeyEvent, app: &mut App, agent_stream: &mut Option<TurnEventStream>) {
    // Overlay-mode key handling: intercept all keys while an overlay is open.
    if matches!(app.view, ViewState::Overlay(_)) {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                app.view = ViewState::Main;
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                // If a turn is running, cancel it; otherwise close overlay.
                if let Some(running) = &app.running {
                    running.cancel.cancel();
                } else {
                    app.view = ViewState::Main;
                }
            }
            _ => {} // Overlays are read-only in v1
        }
        return;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
            if let Some(running) = &app.running {
                // Cancel the active turn; the stream will yield Err(Cancelled).
                running.cancel.cancel();
            } else if app.input.is_empty() {
                app.should_exit = true;
            } else {
                app.input.clear();
                app.cursor = 0;
            }
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => {
            app.should_exit = true;
        }
        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            app.insert_char(c);
        }
        (KeyCode::Backspace, _) => app.backspace(),
        (KeyCode::Delete, _) => app.delete_forward(),
        (KeyCode::Left, _) => app.cursor_left(),
        (KeyCode::Right, _) => app.cursor_right(),
        (KeyCode::Home, _) => app.cursor_home(),
        (KeyCode::End, _) => app.cursor_end(),
        (KeyCode::Up, _) => app.history_up(),
        (KeyCode::Down, _) => app.history_down(),
        (KeyCode::PageUp, _) => {
            app.auto_scroll = false;
            app.scroll = app.scroll.saturating_sub(10);
        }
        (KeyCode::PageDown, _) => {
            app.scroll = app.scroll.saturating_add(10);
        }
        (KeyCode::Enter, _) => {
            let prompt = app.input.trim().to_string();
            if prompt.is_empty() {
                return;
            }
            // Ignore submit if a turn is already running.
            if app.running.is_some() {
                return;
            }

            let line = app.input.clone();
            app.input.clear();
            app.cursor = 0;
            app.history.push(line);
            app.history_index = None;
            app.auto_scroll = true;

            if prompt.starts_with('/') {
                handle_slash_command(&prompt, app);
                return;
            }

            app.transcript
                .push(TranscriptLine::UserPrompt(prompt.clone()));

            // Build message history: prior session messages + new user prompt.
            let mut messages: Vec<caliban_provider::Message> = app
                .session
                .as_ref()
                .map(|s| s.messages.clone())
                .unwrap_or_default();

            // Inject system prompt if not already present in the message list.
            let has_system = messages
                .first()
                .is_some_and(|m| m.role == caliban_provider::Role::System);
            if !has_system {
                if let Some(ref sp) = app.system_prompt {
                    messages.insert(0, caliban_provider::Message::system_text(sp.clone()));
                }
            }

            messages.push(caliban_provider::Message::user_text(prompt));

            // Start the agent stream.
            let cancel = tokio_util::sync::CancellationToken::new();
            let stream = Arc::clone(&app.agent).stream_until_done(messages, cancel.clone());
            app.running = Some(RunningTurn { cancel });
            *agent_stream = Some(stream);
        }
        _ => {}
    }
}
