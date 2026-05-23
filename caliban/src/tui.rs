//! Ratatui-based interactive TUI.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use caliban_agent_core::Agent;
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
use ratatui::widgets::{Paragraph, Wrap};

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
}

impl App {
    /// Construct initial `App` state from CLI args and an optional loaded session.
    pub(crate) fn new(
        agent: Arc<Agent>,
        session: Option<PersistedSession>,
        store: Option<SessionStore>,
        args: Args,
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
            transcript: Vec::new(),
            input: String::new(),
            cursor: 0,
            history,
            history_index: None,
            scroll: 0,
            auto_scroll: true,
            should_exit: false,
            running: None,
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
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(frame.area());

    // Output region
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

    // Status bar
    let status_text = render_status(app);
    let status =
        Paragraph::new(status_text).style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(status, chunks[1]);

    // Input
    let input_line = Line::from(vec![Span::raw("> "), Span::raw(app.input.as_str())]);
    let input_widget = Paragraph::new(input_line);
    frame.render_widget(input_widget, chunks[2]);

    // Cursor positioning: column = 2 ("> " prompt) + char-count up to byte cursor
    let prefix_cols = u16::try_from(app.input[..app.cursor].chars().count()).unwrap_or(u16::MAX);
    frame.set_cursor_position((chunks[2].x + 2 + prefix_cols, chunks[2].y));
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

fn render_status(app: &App) -> String {
    use std::fmt::Write as _;
    let provider = match app.args.provider {
        crate::ProviderKind::Anthropic => "anthropic",
        crate::ProviderKind::Openai => "openai",
        crate::ProviderKind::Ollama => "ollama",
        crate::ProviderKind::Google => "google",
    };
    let model = app
        .args
        .model
        .as_deref()
        .unwrap_or_else(|| crate::default_model_for(app.args.provider));
    let mut s = String::with_capacity(80);
    let _ = write!(s, " {} \u{00B7} {provider} {model}", app.cwd_display());
    if let Some(sess) = &app.session {
        let _ = write!(
            s,
            " \u{00B7} session: {} ({}t)",
            sess.name,
            sess.turn_count()
        );
    }
    if app.running.is_some() {
        s.push_str(" \u{00B7} running\u{2026}");
    }
    s
}

// === Event loop ===

/// Run the TUI until the user exits (Ctrl+D or Ctrl+C with empty input).
///
/// Saves the session on clean exit if `--no-save` was not set.
pub(crate) async fn run(
    args: Args,
    agent: Arc<Agent>,
    store: Option<SessionStore>,
    session: Option<PersistedSession>,
) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new(agent, session, store, args);
    let mut events = EventStream::new();

    loop {
        guard.terminal().draw(|frame| render(frame, &app))?;
        if app.should_exit {
            break;
        }

        // Block on the next terminal event.
        // (Agent integration is T.2; for now this is a simple sequential loop.)
        let event = events.next().await;
        if let Some(Ok(ref ev)) = event {
            handle_event(ev, &mut app);
        } else {
            // Stream ended (terminal closed or error).
            break;
        }
    }

    // Save session on exit
    if let (Some(store), Some(sess)) = (app.store.as_ref(), app.session.as_ref()) {
        if !app.args.no_save {
            let _ = store.save(sess);
        }
    }

    Ok(())
}

fn handle_event(event: &crossterm::event::Event, app: &mut App) {
    use crossterm::event::Event;
    if let Event::Key(key) = event {
        if key.kind != KeyEventKind::Press {
            return;
        }
        handle_key(*key, app);
    }
}

fn handle_key(key: KeyEvent, app: &mut App) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            if app.running.is_some() {
                // Cancel handled in T.2
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
            // T.2 will re-clamp and toggle auto_scroll when near the bottom
        }
        (KeyCode::Enter, _) => {
            let line = app.input.clone();
            app.input.clear();
            app.cursor = 0;
            // T.2 dispatches to handle_submit and starts the agent stream.
            // For T.1, we just echo the input to the transcript.
            if !line.is_empty() {
                app.transcript.push(TranscriptLine::UserPrompt(line));
            }
        }
        // Esc: cancel (implemented in T.2) — fall through to no-op
        _ => {}
    }
}
