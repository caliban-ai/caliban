//! Ratatui-based interactive TUI.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::{Stdout, Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
    System,
}

impl Overlay {
    fn title(self) -> &'static str {
        match self {
            Self::SlashHelp => "Slash Commands",
            Self::Config => "Configuration",
            Self::Mcp => "MCP Servers",
            Self::Skills => "Skills",
            Self::System => "System Prompt",
        }
    }

    fn short_name(self) -> &'static str {
        match self {
            Self::SlashHelp => "help",
            Self::Config => "config",
            Self::Mcp => "mcp",
            Self::Skills => "skills",
            Self::System => "system",
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
use ratatui::widgets::{Block, Borders, Paragraph};

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
    /// What the agent is currently doing — drives the status-bar indicator.
    pub(crate) activity: Activity,
}

/// What phase of work the agent is currently in.
#[derive(Debug, Clone)]
pub(crate) enum Activity {
    /// Submitted; waiting for the provider to respond.
    WaitingForModel { since: std::time::Instant },
    /// Receiving streamed assistant text.
    Streaming { since: std::time::Instant },
    /// Receiving streamed reasoning/thinking output.
    Thinking { since: std::time::Instant },
    /// Dispatching a named tool.
    DispatchingTool {
        name: String,
        since: std::time::Instant,
    },
}

impl Activity {
    fn label(&self) -> String {
        match self {
            Self::WaitingForModel { .. } => "waiting for model".into(),
            Self::Streaming { .. } => "streaming response".into(),
            Self::Thinking { .. } => "thinking".into(),
            Self::DispatchingTool { name, .. } => format!("running {name}"),
        }
    }

    fn since(&self) -> std::time::Instant {
        match self {
            Self::WaitingForModel { since }
            | Self::Streaming { since }
            | Self::Thinking { since }
            | Self::DispatchingTool { since, .. } => *since,
        }
    }
}

/// Pick a frame from a Braille spinner based on elapsed time. ~10 Hz advance.
fn spinner_frame(elapsed: std::time::Duration) -> char {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let idx = (elapsed.as_millis() / 100) as usize % FRAMES.len();
    FRAMES[idx]
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
    /// In-memory message history for the current invocation (ephemeral and session modes).
    pub(crate) messages: Vec<caliban_provider::Message>,
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
        let messages = session
            .as_ref()
            .map(|s| s.messages.clone())
            .unwrap_or_default();
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
            messages,
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
    // Compute how many rows the input area needs based on terminal width.
    // Prompt "> " (2 chars) + input text, character-wrapped at terminal width.
    // Capped at INPUT_MAX_ROWS so a runaway input can't consume the screen.
    const PROMPT_CHARS: usize = 2;
    const INPUT_MAX_ROWS: u16 = 10;
    let area = frame.area();
    let avail_width = area.width as usize;
    let total_input_chars = PROMPT_CHARS + app.input.chars().count();
    let input_rows: u16 = if avail_width == 0 {
        1
    } else {
        let rows = total_input_chars.div_ceil(avail_width).max(1);
        u16::try_from(rows)
            .unwrap_or(INPUT_MAX_ROWS)
            .min(INPUT_MAX_ROWS)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),             // 0: output region (flex)
            Constraint::Length(1),          // 1: top border (horizontal rule)
            Constraint::Length(input_rows), // 2: input area (dynamic; grows with text)
            Constraint::Length(1),          // 3: bottom border
            Constraint::Length(1),          // 4: status bar
        ])
        .split(area);

    // chunks[0] = output region
    //
    // Scroll offset is measured in terminal rows, not Line indices, so we
    // pre-wrap every Line into width-bounded sub-Lines and let lines.len()
    // be the true row count. Doing it ourselves (instead of relying on
    // `Paragraph::wrap`) keeps the math exact AND avoids word-reflow
    // jitter as streaming deltas land mid-word.
    let logical_lines = render_transcript(app);
    let wrapped_lines = wrap_lines_to_width(logical_lines, chunks[0].width);
    let total_rows = u16::try_from(wrapped_lines.len()).unwrap_or(u16::MAX);
    let visible = chunks[0].height;
    let max_scroll = total_rows.saturating_sub(visible);
    let scroll = if app.auto_scroll {
        max_scroll
    } else {
        app.scroll.min(max_scroll)
    };
    let output = Paragraph::new(wrapped_lines).scroll((scroll, 0));
    frame.render_widget(output, chunks[0]);

    // chunks[1] = top horizontal rule
    let hrule_top = Block::default()
        .borders(Borders::TOP)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hrule_top, chunks[1]);

    // chunks[2] = input — character-wrapped manually so cursor math stays aligned.
    let input_chunk_width = chunks[2].width as usize;
    if input_chunk_width > 0 {
        let combined: String = {
            let mut s = String::with_capacity(PROMPT_CHARS + app.input.len());
            s.push_str("> ");
            s.push_str(&app.input);
            s
        };
        let chars: Vec<char> = combined.chars().collect();
        let mut input_lines: Vec<Line<'_>> = Vec::new();
        let mut idx = 0;
        while idx < chars.len() {
            let end = (idx + input_chunk_width).min(chars.len());
            let chunk: String = chars[idx..end].iter().collect();
            input_lines.push(Line::raw(chunk));
            idx = end;
        }
        if input_lines.is_empty() {
            input_lines.push(Line::raw("> "));
        }
        frame.render_widget(Paragraph::new(input_lines), chunks[2]);
    }

    // chunks[3] = bottom horizontal rule
    let hrule_bot = Block::default()
        .borders(Borders::TOP)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hrule_bot, chunks[3]);

    // chunks[4] = status bar
    let status = render_status(app);
    frame.render_widget(Paragraph::new(status), chunks[4]);

    // Cursor position — in chunks[2], accounting for character-wrap.
    if let Some(width_nz) = std::num::NonZeroUsize::new(input_chunk_width) {
        let prefix_chars = PROMPT_CHARS + app.input[..app.cursor].chars().count();
        let cursor_row = u16::try_from(prefix_chars / width_nz)
            .unwrap_or(INPUT_MAX_ROWS)
            .min(input_rows.saturating_sub(1));
        let cursor_col = u16::try_from(prefix_chars % width_nz).unwrap_or(0);
        frame.set_cursor_position((chunks[2].x + cursor_col, chunks[2].y + cursor_row));
    }

    // Render overlay on top if one is active.
    if let ViewState::Overlay(o) = app.view {
        render_overlay(frame, app, o);
    }
}

#[allow(clippy::too_many_lines)]
fn format_tool_input(input: &str, max_chars: usize) -> String {
    use serde_json::Value;
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(map)) => {
            let mut parts: Vec<String> = Vec::with_capacity(map.len());
            for (k, v) in &map {
                let v_str = match v {
                    Value::String(s) => {
                        if s.chars().count() > 40 {
                            let truncated: String = s.chars().take(40).collect();
                            format!("\"{truncated}\u{2026}\"")
                        } else {
                            format!("\"{s}\"")
                        }
                    }
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Null => "null".to_string(),
                    other => other.to_string(),
                };
                parts.push(format!("{k}={v_str}"));
            }
            let joined = parts.join(", ");
            if joined.chars().count() > max_chars {
                let truncated: String = joined.chars().take(max_chars).collect();
                format!("{truncated}\u{2026}")
            } else {
                joined
            }
        }
        _ => {
            if input.chars().count() > max_chars {
                let truncated: String = input.chars().take(max_chars).collect();
                format!("{truncated}\u{2026}")
            } else {
                input.to_string()
            }
        }
    }
}

/// Pre-wrap a vector of styled `Line`s into width-bounded visual rows,
/// preserving each span's style.
///
/// We do this ourselves (rather than relying on `Paragraph::wrap`) so the
/// resulting `Vec::len()` is the *exact* number of terminal rows the
/// transcript occupies, which lets the auto-scroll offset stay accurate. We
/// also avoid the word-reflow jitter that `Wrap { trim: false }` produces
/// when streaming deltas land mid-word.
///
/// Wrap point is char count, not unicode-width display columns — matching
/// the input area's hand-rolled wrap, and good enough for the predominantly
/// ASCII transcript. Fix in a follow-up if multi-width glyphs land in
/// output frequently.
fn wrap_lines_to_width<'a>(lines: Vec<Line<'a>>, width: u16) -> Vec<Line<'a>> {
    if width == 0 {
        return lines;
    }
    let width = width as usize;
    let mut out: Vec<Line<'a>> = Vec::with_capacity(lines.len());
    for line in lines {
        let line_style = line.style;
        let line_align = line.alignment;
        let mut row: Vec<Span<'a>> = Vec::new();
        let mut row_chars: usize = 0;
        let mut emitted_any = false;
        for span in line.spans {
            let style = span.style;
            let chars: Vec<char> = span.content.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let avail = width - row_chars;
                let take = avail.min(chars.len() - i);
                let chunk: String = chars[i..i + take].iter().collect();
                row.push(Span::styled(chunk, style));
                row_chars += take;
                i += take;
                if row_chars == width {
                    let mut emitted = Line::from(std::mem::take(&mut row));
                    emitted.style = line_style;
                    emitted.alignment = line_align;
                    out.push(emitted);
                    row_chars = 0;
                    emitted_any = true;
                }
            }
        }
        if !row.is_empty() || !emitted_any {
            let mut emitted = Line::from(std::mem::take(&mut row));
            emitted.style = line_style;
            emitted.alignment = line_align;
            out.push(emitted);
        }
    }
    out
}

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
                let input_summary = format_tool_input(input, 80);
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
        Overlay::System => system_lines(app),
    };

    let body = Paragraph::new(content_lines).wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

fn slash_help_lines() -> Vec<Line<'static>> {
    let entries = [
        ("/help", "Show this help"),
        ("/exit, /quit", "Save session and exit"),
        (
            "/clear",
            "Clear transcript AND in-memory history (session messages cleared too)",
        ),
        ("/sessions", "List saved sessions"),
        ("/save [<name>]", "Save current session (optionally rename)"),
        ("/usage", "Show accumulated usage"),
        ("/config", "Show active configuration"),
        ("/mcp", "MCP server configuration (stub)"),
        ("/skills", "Skills configuration (stub)"),
        ("/system", "View current system prompt"),
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

fn system_lines(app: &App) -> Vec<Line<'static>> {
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
    let running_part = if let Some(running) = &app.running {
        let elapsed = running.activity.since().elapsed();
        let secs = elapsed.as_secs();
        let spinner = spinner_frame(elapsed);
        format!(
            " \u{00B7} {spinner} {} ({}s)",
            running.activity.label(),
            secs,
        )
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
    tracing::debug!(?evt, "agent event");
    match evt {
        TurnEvent::TurnStart { .. } => {
            // Keep the WaitingForModel activity; refreshed on first delta below.
        }
        TurnEvent::AssistantTextDelta { text, .. } => {
            // First delta of this stream → transition to Streaming activity.
            if let Some(running) = app.running.as_mut() {
                if !matches!(running.activity, Activity::Streaming { .. }) {
                    running.activity = Activity::Streaming {
                        since: std::time::Instant::now(),
                    };
                }
            }
            // Find or create the in-progress AssistantText line.
            if let Some(TranscriptLine::AssistantText(buf)) = app.transcript.last_mut() {
                buf.push_str(&text);
            } else {
                app.transcript.push(TranscriptLine::AssistantText(text));
            }
        }
        TurnEvent::AssistantThinkingDelta { text, .. } => {
            if let Some(running) = app.running.as_mut() {
                if !matches!(running.activity, Activity::Thinking { .. }) {
                    running.activity = Activity::Thinking {
                        since: std::time::Instant::now(),
                    };
                }
            }
            if let Some(TranscriptLine::AssistantThinking(buf)) = app.transcript.last_mut() {
                buf.push_str(&text);
            } else {
                app.transcript.push(TranscriptLine::AssistantThinking(text));
            }
        }
        TurnEvent::ToolCallStart {
            tool_use_id, name, ..
        } => {
            if let Some(running) = app.running.as_mut() {
                running.activity = Activity::DispatchingTool {
                    name: name.clone(),
                    since: std::time::Instant::now(),
                };
            }
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
            // Tool finished; back to "waiting for model" (next tool dispatch or
            // the next provider call will update this further).
            if let Some(running) = app.running.as_mut() {
                running.activity = Activity::WaitingForModel {
                    since: std::time::Instant::now(),
                };
            }
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
            // Tool dispatch (sequential) and the next turn's provider call are
            // about to happen — show "waiting for model" until the next event.
            if let Some(running) = app.running.as_mut() {
                running.activity = Activity::WaitingForModel {
                    since: std::time::Instant::now(),
                };
            }
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
            // Update in-memory history (works for both ephemeral and session modes).
            app.messages.clone_from(&final_messages);
            // Persist to session if applicable (consumes final_messages).
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
    tracing::warn!(error = %e, "agent error");
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

    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        guard.terminal().draw(|frame| render(frame, &app))?;
        tracing::trace!("draw");
        stdout().flush().ok();
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
            _ = tick.tick() => {
                // No-op; the loop will redraw on next iteration.
            }
        }

        tokio::task::yield_now().await;
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
        "/system" => {
            app.view = ViewState::Overlay(Overlay::System);
        }
        "/exit" | "/quit" => {
            app.should_exit = true;
        }
        "/clear" => {
            app.transcript.clear();
            app.messages.clear();
            // Clear session messages too if applicable; the next save overwrites.
            if let Some(sess) = app.session.as_mut() {
                sess.messages.clear();
            }
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
    tracing::trace!(?event, "term event");
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

            // Build message history: in-memory history + new user prompt.
            let mut messages: Vec<caliban_provider::Message> = app.messages.clone();

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
            app.running = Some(RunningTurn {
                cancel,
                activity: Activity::WaitingForModel {
                    since: std::time::Instant::now(),
                },
            });
            *agent_stream = Some(stream);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_short_line_unchanged() {
        let input = vec![Line::raw("hello")];
        let out = wrap_lines_to_width(input, 10);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn wrap_long_line_splits_into_correct_row_count() {
        // 25 chars in a width-10 region => ceil(25/10) = 3 rows.
        let input = vec![Line::raw("abcdefghijklmnopqrstuvwxy")];
        let out = wrap_lines_to_width(input, 10);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn wrap_preserves_empty_lines() {
        // Important: empty separator lines must produce one visual row each.
        let input = vec![Line::raw(""), Line::raw("x"), Line::raw("")];
        let out = wrap_lines_to_width(input, 10);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn wrap_preserves_span_styles_across_split() {
        // Mixed-style Line: bold "user: " label + plain text that wraps.
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let input = vec![Line::from(vec![
            Span::styled("user: ", bold),
            Span::raw("abcdefghij"), // 10 chars; width=8 => spills onto row 2
        ])];
        let out = wrap_lines_to_width(input, 8);
        assert_eq!(out.len(), 2);
        // First row keeps the bold "user: " styling on its first span.
        assert!(out[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        // The plain-text spans on either row are not bold.
        let last = out[1].spans.last().expect("row 2 has content");
        assert!(!last.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn wrap_exact_multiple_of_width_does_not_emit_extra_blank() {
        // 10 chars at width 5 => exactly 2 rows, not 3.
        let input = vec![Line::raw("0123456789")];
        let out = wrap_lines_to_width(input, 5);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn wrap_zero_width_returns_input_unchanged() {
        let input = vec![Line::raw("hello"), Line::raw("world")];
        let out = wrap_lines_to_width(input.clone(), 0);
        assert_eq!(out.len(), input.len());
    }
}
