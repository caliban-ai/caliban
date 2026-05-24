//! Ratatui-based interactive TUI.

#![allow(clippy::print_stdout, clippy::print_stderr)]

mod attach;
mod completer;
mod input;
mod toast;

use input::InputMode;

/// Slash-command names + their literal insert text. Used by the slash menu
/// to populate candidates and by `handle_key` to detect the trigger.
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "/help"),
    ("/clear", "/clear"),
    ("/config", "/config"),
    ("/mcp", "/mcp"),
    ("/skills", "/skills"),
    ("/system", "/system"),
    ("/sessions", "/sessions"),
    ("/save", "/save"),
    ("/usage", "/usage"),
    ("/memory", "/memory"),
    ("/plan", "/plan"),
    ("/hooks", "/hooks"),
    ("/exit", "/exit"),
    ("/quit", "/quit"),
];

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
    #[allow(
        dead_code,
        reason = "Overlay::Skills retained for slash menu enum parity; /skills now prints to transcript"
    )]
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
    event::{
        DisableMouseCapture, EnableMouseCapture, EventStream, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::Args;

/// RAII guard that enables raw mode + alternate screen on creation,
/// and restores the terminal on drop (including on panic).
pub(crate) struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    /// Enter raw mode and the alternate screen, constructing the guard.
    ///
    /// Mouse capture is enabled so the scroll wheel reaches the app. Side
    /// effect: native click-to-select stops working in the alternate screen
    /// — most macOS terminals offer Option-drag (iTerm2, macOS Terminal) or
    /// Shift-drag (xterm-likes) as the escape hatch.
    pub(crate) fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        // Best-effort: kitty keyboard protocol lets us distinguish
        // Shift+Enter from plain Enter on supporting terminals (kitty,
        // iTerm2 with modifier reporting, Ghostty, foot, WezTerm).
        // Legacy terminals ignore the push silently — Alt+Enter is the
        // documented portable fallback for inserting a newline.
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
            ),
        );
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
        // Restore in reverse order of acquisition. DisableMouseCapture and
        // PopKeyboardEnhancementFlags are best-effort: if they fail the
        // terminal eventually clears state on its own.
        let _ = execute!(
            stdout(),
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            LeaveAlternateScreen,
        );
        let _ = disable_raw_mode();
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
        /// Total tokens read from the prompt cache (`Anthropic` from turn 2
        /// onward; `OpenAI` on prompts >=1024 tokens). `None` when no cache
        /// info was returned by the provider.
        cache_read: Option<u32>,
        /// Total tokens written to the prompt cache (`Anthropic` only, first
        /// turn surcharge). `None` for providers that don't report it.
        cache_creation: Option<u32>,
        /// Most recent turn's time-to-first-token in milliseconds. Wired in
        /// the TTFT slice.
        last_turn_ttft_ms: Option<u64>,
        /// Number of agent turns taken.
        turn_count: u32,
    },
    /// Informational message (session save, slash-command output, …).
    Info(String),
    /// Error message.
    Error(String),
    /// 📎 marker shown under a user prompt for each `@`-attached file.
    Attached {
        /// Path the model sees in the `--- attached: ... ---` framing.
        display_path: String,
        /// Byte size of the attached content.
        bytes: u64,
    },
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
    /// Input area state (text, cursor, history, mode).
    pub(crate) input: input::Input,
    /// Manual scroll offset (rows from top) when `auto_scroll` is false.
    pub(crate) scroll: u16,
    /// When `true`, keep the viewport pinned to the bottom of the transcript.
    pub(crate) auto_scroll: bool,
    /// Largest valid manual-scroll offset, updated each render. Used by the
    /// mouse wheel handler so a "scroll back down past the end" re-enables
    /// auto-scroll without recomputing wrap widths in the event path.
    pub(crate) last_max_scroll: u16,
    /// Set to `true` to break the event loop cleanly.
    pub(crate) should_exit: bool,
    /// Non-`None` while an agent turn is in progress.
    pub(crate) running: Option<RunningTurn>,
    /// Current view state: main view or an open overlay.
    pub(crate) view: ViewState,
    /// In-memory message history for the current invocation (ephemeral and session modes).
    pub(crate) messages: Vec<caliban_provider::Message>,
    /// Ephemeral toast shown above the input area (5s TTL or until next key).
    pub(crate) toast: Option<toast::Toast>,
    /// Most recent turn's time-to-first-token in milliseconds. Populated on
    /// each `TurnEvent::TurnEnd` (wired in TTFT slice); consumed by `RunEnd`
    /// when building the `UsageSummary` transcript line, then reset.
    pub(crate) last_turn_ttft_ms: Option<u64>,
    /// Shared handle to the canonical todo list. Mutated by `TodoWriteTool`;
    /// snapshotted into `session.todos` on save; spliced into the system
    /// prompt at the start of every user-driven turn.
    pub(crate) todos: caliban_agent_core::SharedTodos,
    /// Shared plan-mode flag. Toggled by `/plan` and by the
    /// `EnterPlanMode`/`ExitPlanMode` tools.
    pub(crate) plan_mode: caliban_agent_core::SharedPlanMode,
}

impl App {
    /// Construct initial `App` state from CLI args and an optional loaded session.
    pub(crate) fn new(
        agent: Arc<Agent>,
        session: Option<PersistedSession>,
        store: Option<SessionStore>,
        args: Args,
        system_prompt: Option<String>,
        todos: caliban_agent_core::SharedTodos,
        plan_mode: caliban_agent_core::SharedPlanMode,
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
            input: input::Input::from_history(history),
            scroll: 0,
            auto_scroll: true,
            last_max_scroll: 0,
            should_exit: false,
            running: None,
            view: ViewState::Main,
            messages,
            toast: None,
            last_turn_ttft_ms: None,
            todos,
            plan_mode,
        }
    }

    /// Return the current working directory as a tilde-collapsed display string.
    pub(crate) fn cwd_display(&self) -> String {
        if let Some(home) = dirs::home_dir()
            && let Ok(stripped) = self.cwd.strip_prefix(&home)
        {
            if stripped.as_os_str().is_empty() {
                return "~".into();
            }
            return format!("~/{}", stripped.display());
        }
        self.cwd.display().to_string()
    }
}

// === Rendering ===

#[allow(clippy::too_many_lines)]
fn render(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    // Compute how many rows the input area needs based on terminal width.
    // Prompt "> " (2 chars) + input text, character-wrapped at terminal width.
    // Capped at INPUT_MAX_ROWS so a runaway input can't consume the screen.
    const PROMPT_CHARS: usize = 2;
    const INPUT_MAX_ROWS: u16 = 10;
    let area = frame.area();
    let avail_width = area.width as usize;
    // Count visible rows: each '\n' starts a new logical line, then each
    // logical line wraps at avail_width. First logical line carries the
    // "> " prompt; subsequent ones (after Shift+Enter) do not.
    let input_rows: u16 = if avail_width == 0 {
        1
    } else {
        let mut total: usize = 0;
        for (i, segment) in app.input.buffer.split('\n').enumerate() {
            let chars = segment.chars().count() + if i == 0 { PROMPT_CHARS } else { 0 };
            total += chars.div_ceil(avail_width).max(1);
        }
        u16::try_from(total.clamp(1, INPUT_MAX_ROWS as usize)).unwrap_or(INPUT_MAX_ROWS)
    };

    let toast_rows: u16 = u16::from(app.toast.as_ref().is_some_and(|t| !t.is_expired()));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),             // 0: output region (flex)
            Constraint::Length(1),          // 1: top border (horizontal rule)
            Constraint::Length(toast_rows), // 2: toast strip (0 when no toast)
            Constraint::Length(input_rows), // 3: input area (dynamic; grows with text)
            Constraint::Length(1),          // 4: bottom border
            Constraint::Length(1),          // 5: status bar
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
        // Clamp in case the transcript shrank under a manually-set offset
        // (e.g. /clear while scrolled up).
        app.scroll.min(max_scroll)
    };
    let output = Paragraph::new(wrapped_lines).scroll((scroll, 0));
    frame.render_widget(output, chunks[0]);
    // Commit derived state to app. Safe to mutate now — wrapped_lines and
    // its borrows on `app.transcript` were consumed by render_widget above.
    app.last_max_scroll = max_scroll;
    app.scroll = scroll;

    // chunks[1] = top horizontal rule
    let hrule_top = Block::default()
        .borders(Borders::TOP)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hrule_top, chunks[1]);

    // chunks[2] = ephemeral toast (zero rows when no toast is active).
    if toast_rows == 1
        && let Some(t) = &app.toast
    {
        let (fg, bg) = match t.level {
            toast::ToastLevel::Error => (Color::White, Color::Red),
            toast::ToastLevel::Warn => (Color::Black, Color::Yellow),
            toast::ToastLevel::Info => (Color::Gray, Color::Reset),
        };
        let line = Paragraph::new(Line::from(Span::styled(
            t.text.clone(),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )));
        frame.render_widget(line, chunks[2]);
    }

    // chunks[3] = input — character-wrapped manually so cursor math stays aligned.
    // We split on '\n' first so multi-line composition (Shift+Enter) gets one
    // logical row per segment, each then char-wrapped at chunk width.
    let input_chunk_width = chunks[3].width as usize;
    if input_chunk_width > 0 {
        let mut input_lines: Vec<Line<'_>> = Vec::new();
        for (seg_idx, segment) in app.input.buffer.split('\n').enumerate() {
            let mut s = String::new();
            if seg_idx == 0 {
                s.push_str("> ");
            }
            s.push_str(segment);
            let chars: Vec<char> = s.chars().collect();
            if chars.is_empty() {
                input_lines.push(Line::raw(""));
                continue;
            }
            let mut idx = 0;
            while idx < chars.len() {
                let end = (idx + input_chunk_width).min(chars.len());
                let chunk: String = chars[idx..end].iter().collect();
                input_lines.push(Line::raw(chunk));
                idx = end;
            }
        }
        if input_lines.is_empty() {
            input_lines.push(Line::raw("> "));
        }
        frame.render_widget(Paragraph::new(input_lines), chunks[3]);
    }

    // Slash/At completion menu floats just above the input area.
    match app.input.mode {
        InputMode::SlashMenu(ref menu) | InputMode::AtMenu(ref menu) => {
            render_input_menu(frame, chunks[3], menu);
        }
        InputMode::Idle => {}
    }

    // chunks[4] = bottom horizontal rule
    let hrule_bot = Block::default()
        .borders(Borders::TOP)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hrule_bot, chunks[4]);

    // chunks[5] = status bar
    let status = render_status(app);
    frame.render_widget(Paragraph::new(status), chunks[5]);

    // Cursor position — in chunks[3], accounting for char-wrap AND newlines.
    if let Some(width_nz) = std::num::NonZeroUsize::new(input_chunk_width) {
        let width = width_nz.get();
        let before = &app.input.buffer[..app.input.cursor];
        let segments: Vec<&str> = before.split('\n').collect();
        // Each '\n' before the cursor starts a fresh logical line. Char-wrap
        // each completed segment to count its visual rows; the last (active)
        // segment determines the cursor's row/col within it.
        let mut row: usize = 0;
        let last_idx = segments.len() - 1;
        for (i, seg) in segments.iter().enumerate() {
            let chars = seg.chars().count() + if i == 0 { PROMPT_CHARS } else { 0 };
            if i < last_idx {
                row += chars.div_ceil(width).max(1);
            } else {
                row += chars / width;
            }
        }
        let last_seg_chars =
            segments[last_idx].chars().count() + if last_idx == 0 { PROMPT_CHARS } else { 0 };
        let col = last_seg_chars % width;
        let cursor_row = u16::try_from(row)
            .unwrap_or(INPUT_MAX_ROWS)
            .min(input_rows.saturating_sub(1));
        let cursor_col = u16::try_from(col).unwrap_or(0);
        frame.set_cursor_position((chunks[3].x + cursor_col, chunks[3].y + cursor_row));
    }

    // Render overlay on top if one is active.
    if let ViewState::Overlay(o) = app.view {
        render_overlay(frame, app, o);
    }
}

/// Float a completion menu directly above the input area. Capped at 8 rows;
/// the highlighted item gets a cyan background. Drawn with `Clear` so it
/// overwrites the transcript rows it floats over.
fn render_input_menu(frame: &mut ratatui::Frame<'_>, input_area: Rect, menu: &input::MenuState) {
    if menu.candidates.is_empty() {
        return;
    }
    let max_rows: u16 = 8;
    let height = u16::try_from(menu.candidates.len())
        .unwrap_or(max_rows)
        .min(max_rows);
    // +2 for top/bottom borders of the list block.
    let outer_height = height.saturating_add(2);
    let y = input_area.y.saturating_sub(outer_height);
    let menu_area = Rect {
        x: input_area.x,
        y,
        width: input_area.width,
        height: outer_height,
    };
    let items: Vec<ListItem<'_>> = menu
        .candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let style = if i == menu.selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(c.display.clone(), style)))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(Clear, menu_area);
    frame.render_widget(list, menu_area);
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

#[allow(clippy::too_many_lines)]
fn render_transcript(app: &App) -> Vec<Line<'_>> {
    let mut lines: Vec<Line<'_>> = Vec::new();
    for entry in &app.transcript {
        match entry {
            TranscriptLine::UserPrompt(s) => {
                // Multi-line composition (Shift/Alt+Enter) embeds '\n' in
                // the buffer. Each segment becomes its own Line; the first
                // gets the "user:" label, the rest are indented to align.
                let label = Span::styled(
                    "user: ",
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .fg(Color::Cyan),
                );
                for (i, segment) in s.split('\n').enumerate() {
                    if i == 0 {
                        lines.push(Line::from(vec![
                            label.clone(),
                            Span::raw(segment.to_string()),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::raw("      "), // align with "user: " prefix (6 chars)
                            Span::raw(segment.to_string()),
                        ]));
                    }
                }
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
                cache_read,
                cache_creation,
                last_turn_ttft_ms,
                turn_count,
            } => {
                let mut parts: Vec<String> = Vec::new();
                parts.push(format!("{turn_count} turns"));
                let cache_suffix = format_cache_suffix(*cache_read, *cache_creation);
                parts.push(format!(
                    "{input_tokens}\u{2191}{cache_suffix} {output_tokens}\u{2193} tokens"
                ));
                if let Some(ttft) = last_turn_ttft_ms {
                    parts.push(format!("TTFT {ttft}ms"));
                }
                lines.push(Line::styled(
                    format!("[caliban: {}]", parts.join(" \u{00B7} ")),
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
            TranscriptLine::Attached {
                display_path,
                bytes,
            } => {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        format!("📎 {display_path} ({})", format_bytes(*bytes)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }
    }
    lines
}

/// Human-readable byte size for the 📎 attachment line.
fn format_bytes(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Suffix appended to the input-token count in the `UsageSummary` line when
/// prompt-cache stats are present. Empty string when both counters are zero
/// or absent. Format:
/// - read only:    ` (42 cached)`
/// - write only:   ` (100 cache write)`
/// - both:         ` (42 cached, 100 write)`
fn format_cache_suffix(cache_read: Option<u32>, cache_creation: Option<u32>) -> String {
    let r = cache_read.unwrap_or(0);
    let c = cache_creation.unwrap_or(0);
    match (r, c) {
        (0, 0) => String::new(),
        (r, 0) => format!(" ({r} cached)"),
        (0, c) => format!(" ({c} cache write)"),
        (r, c) => format!(" ({r} cached, {c} write)"),
    }
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
        ("/hooks", "Configured hooks summary (stub)"),
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
    let plan_part = if app.plan_mode.load(std::sync::atomic::Ordering::Relaxed) {
        " \u{00B7} [\u{1F4CB} plan]"
    } else {
        ""
    };

    let text = format!(
        " {cwd} \u{00B7} {provider} {model}{session_part}{plan_part}{overlay_part}{running_part}"
    );
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
            if let Some(running) = app.running.as_mut()
                && !matches!(running.activity, Activity::Streaming { .. })
            {
                running.activity = Activity::Streaming {
                    since: std::time::Instant::now(),
                };
            }
            // Find or create the in-progress AssistantText line.
            if let Some(TranscriptLine::AssistantText(buf)) = app.transcript.last_mut() {
                buf.push_str(&text);
            } else {
                app.transcript.push(TranscriptLine::AssistantText(text));
            }
        }
        TurnEvent::AssistantThinkingDelta { text, .. } => {
            if let Some(running) = app.running.as_mut()
                && !matches!(running.activity, Activity::Thinking { .. })
            {
                running.activity = Activity::Thinking {
                    since: std::time::Instant::now(),
                };
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
                    && *id == tool_use_id
                {
                    input.push_str(&partial_json);
                    break;
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
                    && *id == tool_use_id
                {
                    *result = Some((is_error, result_text));
                    break;
                }
            }
        }
        TurnEvent::TurnEnd { ttft, .. } => {
            if let Some(t) = ttft {
                let millis = u64::try_from(t.as_millis()).unwrap_or(u64::MAX);
                app.last_turn_ttft_ms = Some(millis);
            }
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
                cache_read: total_usage.cache_read_input_tokens,
                cache_creation: total_usage.cache_creation_input_tokens,
                last_turn_ttft_ms: app.last_turn_ttft_ms,
                turn_count,
            });
            // Reset for the next run (a /clear + new prompt should start fresh).
            app.last_turn_ttft_ms = None;
            // Update in-memory history (works for both ephemeral and session modes).
            app.messages.clone_from(&final_messages);
            // Persist to session if applicable (consumes final_messages).
            if let Some(sess) = app.session.as_mut() {
                sess.merge_run(final_messages, total_usage);
                // Snapshot the live todo handle into the session for persistence.
                sess.todos
                    .clone_from(&*app.todos.lock().expect("todos lock poisoned"));
                sess.plan_mode = app.plan_mode.load(std::sync::atomic::Ordering::Relaxed);
                if let Some(store) = app.store.as_ref()
                    && !app.args.no_save
                {
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
    todos: caliban_agent_core::SharedTodos,
    plan_mode: caliban_agent_core::SharedPlanMode,
) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new(agent, session, store, args, system_prompt, todos, plan_mode);
    let mut events = EventStream::new();
    let mut agent_stream: Option<TurnEventStream> = None;

    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Drop expired toast before drawing so it doesn't flicker for one tick.
        if app.toast.as_ref().is_some_and(toast::Toast::is_expired) {
            app.toast = None;
        }
        guard.terminal().draw(|frame| render(frame, &mut app))?;
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
    if let (Some(store), Some(sess)) = (app.store.as_ref(), app.session.as_ref())
        && !app.args.no_save
    {
        let _ = store.save(sess);
    }

    // Fire SessionEnd (best-effort).
    {
        let session_id = app
            .args
            .session
            .clone()
            .unwrap_or_else(|| "tui-ephemeral".into());
        let cwd = app.cwd.clone();
        let provider = "tui";
        let model = app.args.model.clone().unwrap_or_default();
        let session_ctx = caliban_agent_core::SessionCtx {
            session_id: &session_id,
            cwd: &cwd,
            provider,
            model: &model,
        };
        let total_in = app
            .session
            .as_ref()
            .map_or(0, |s| s.total_usage.input_tokens);
        let total_out = app
            .session
            .as_ref()
            .map_or(0, |s| s.total_usage.output_tokens);
        let outcome = caliban_agent_core::SessionOutcome {
            turn_count: app
                .session
                .as_ref()
                .map_or(0, caliban_sessions::PersistedSession::turn_count),
            input_tokens: total_in,
            output_tokens: total_out,
        };
        if let Err(e) = app.agent.hooks().session_end(&session_ctx, &outcome).await {
            tracing::warn!(target: "caliban::hooks", error = %e, "session_end hook error (non-fatal)");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
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
            let workspace_root = app
                .args
                .workspace
                .clone()
                .unwrap_or_else(|| app.cwd.clone());
            let roots = caliban_skills::default_roots(&workspace_root);
            let skills = caliban_skills::load_skills(&roots);
            if skills.is_empty() {
                app.transcript.push(TranscriptLine::Info(
                    "no skills loaded (drop a SKILL.md under .caliban/skills/<name>/)".into(),
                ));
            } else {
                app.transcript.push(TranscriptLine::Info(format!(
                    "{} skill(s) loaded:",
                    skills.len()
                )));
                for s in &skills {
                    let first = s.description.lines().next().unwrap_or("");
                    app.transcript
                        .push(TranscriptLine::Info(format!("  {} — {}", s.name, first)));
                }
            }
        }
        "/system" => {
            app.view = ViewState::Overlay(Overlay::System);
        }
        "/hooks" => {
            // Stub overlay (the proper /hooks UI lands with ADR 0040).
            // For v1, we render a one-line summary line per configured event.
            let workspace_root = app
                .args
                .workspace
                .clone()
                .unwrap_or_else(|| app.cwd.clone());
            let cfg = caliban_agent_core::HooksConfig::load(&workspace_root).unwrap_or_default();
            if cfg.total_handler_count() == 0 {
                app.transcript.push(TranscriptLine::Info(
                    "no hooks configured (drop a hooks.toml under .caliban/ or ~/.config/caliban/)"
                        .into(),
                ));
            } else {
                app.transcript.push(TranscriptLine::Info(format!(
                    "{} hook handler(s) loaded across {} event(s):",
                    cfg.total_handler_count(),
                    cfg.events.len()
                )));
                for (event, handlers) in &cfg.events {
                    app.transcript.push(TranscriptLine::Info(format!(
                        "  {event} → {} handler(s)",
                        handlers.len()
                    )));
                }
                if cfg.disable_all_hooks {
                    app.transcript.push(TranscriptLine::Info(
                        "kill switch active (disable_all_hooks = true)".into(),
                    ));
                }
            }
        }
        "/exit" | "/quit" => {
            app.should_exit = true;
        }
        "/clear" => {
            app.transcript.clear();
            app.messages.clear();
            app.last_turn_ttft_ms = None;
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
        "/plan" => {
            use std::sync::atomic::Ordering;
            let now = !app.plan_mode.load(Ordering::Relaxed);
            app.plan_mode.store(now, Ordering::Relaxed);
            if let Some(sess) = app.session.as_mut() {
                sess.plan_mode = now;
            }
            let msg = if now {
                "plan mode: ON — mutating tools blocked until /plan toggles off"
            } else {
                "plan mode: OFF — mutating tools available"
            };
            app.transcript.push(TranscriptLine::Info(msg.into()));
        }
        "/memory" => {
            let workspace_root = app
                .args
                .workspace
                .clone()
                .unwrap_or_else(|| app.cwd.clone());
            let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);
            // We block the event loop for one fs read; tiers are small.
            let prefix = futures::executor::block_on(caliban_memory::load(&cfg));
            match prefix {
                Ok(p) => {
                    app.transcript.push(TranscriptLine::Info(format!(
                        "memory tiers ({} tokens / {} budget):",
                        p.estimated_tokens, cfg.max_tokens
                    )));
                    for line in p.summary_lines() {
                        app.transcript.push(TranscriptLine::Info(line));
                    }
                    if p.truncated {
                        app.transcript.push(TranscriptLine::Info(
                            "(some tiers truncated — raise CALIBAN_MEMORY_BUDGET_TOKENS or trim)"
                                .into(),
                        ));
                    }
                }
                Err(e) => {
                    app.transcript
                        .push(TranscriptLine::Error(format!("memory load failed: {e}")));
                }
            }
        }
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
    match event {
        Event::Key(key) => {
            if key.kind != KeyEventKind::Press {
                return;
            }
            handle_key(*key, app, agent_stream);
        }
        Event::Mouse(mouse) => handle_mouse(*mouse, app),
        _ => {}
    }
}

/// Rows of scroll per wheel notch. Three is what most terminals give you
/// natively in their scroll-back and matches the cadence in `PageUp` (10
/// felt too aggressive for a fine-grained wheel).
const MOUSE_WHEEL_ROWS: u16 = 3;

fn handle_mouse(event: MouseEvent, app: &mut App) {
    // Overlays are short static content — ignore wheel inside them rather
    // than confusing the user by silently scrolling the transcript behind.
    if matches!(app.view, ViewState::Overlay(_)) {
        return;
    }
    match event.kind {
        MouseEventKind::ScrollUp => {
            // When transitioning out of auto-scroll, seed app.scroll from
            // the current bottom so the first wheel tick actually steps up
            // from where the user was looking — not from a stale offset.
            if app.auto_scroll {
                app.scroll = app.last_max_scroll;
                app.auto_scroll = false;
            }
            app.scroll = app.scroll.saturating_sub(MOUSE_WHEEL_ROWS);
        }
        MouseEventKind::ScrollDown => {
            let next = app.scroll.saturating_add(MOUSE_WHEEL_ROWS);
            if next >= app.last_max_scroll {
                // Scrolled past the end → re-pin to live tail.
                app.scroll = app.last_max_scroll;
                app.auto_scroll = true;
            } else {
                app.scroll = next;
            }
        }
        // Clicks, drags, motion — intentionally ignored. We capture the
        // mouse only so the wheel reaches us.
        _ => {}
    }
}

#[allow(clippy::too_many_lines)]
fn handle_key(key: KeyEvent, app: &mut App, agent_stream: &mut Option<TurnEventStream>) {
    // Any keystroke dismisses an active toast — but the keystroke itself
    // still takes effect below.
    if app.toast.is_some() {
        app.toast = None;
    }

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

    // Menu-mode navigation (Tab / arrows / Enter / Esc) intercepts before
    // the normal input dispatch. Printable characters and Backspace fall
    // through to the regular handlers; the buffer is then re-evaluated for
    // menu state at the end of this function.
    if matches!(
        app.input.mode,
        InputMode::SlashMenu(_) | InputMode::AtMenu(_)
    ) {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                app.input.close_menu();
                return;
            }
            (KeyCode::Tab | KeyCode::Down, _) => {
                if let InputMode::SlashMenu(ref mut m) | InputMode::AtMenu(ref mut m) =
                    app.input.mode
                {
                    m.cycle_next();
                }
                return;
            }
            (KeyCode::BackTab | KeyCode::Up, _) => {
                if let InputMode::SlashMenu(ref mut m) | InputMode::AtMenu(ref mut m) =
                    app.input.mode
                {
                    m.cycle_prev();
                }
                return;
            }
            (KeyCode::Enter, m)
                if !m.contains(KeyModifiers::SHIFT) && !m.contains(KeyModifiers::ALT) =>
            {
                let was_at = matches!(app.input.mode, InputMode::AtMenu(_));
                let was_dir = app.input.accept_menu_selection();
                if was_at && was_dir {
                    // Selecting a directory leaves cursor after `src/`;
                    // re-open the menu showing the new directory contents.
                    refresh_at_menu(app);
                }
                return;
            }
            _ => {}
        }
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
            if let Some(running) = &app.running {
                // Cancel the active turn; the stream will yield Err(Cancelled).
                running.cancel.cancel();
            } else if app.input.buffer.is_empty() {
                app.should_exit = true;
            } else {
                app.input.clear();
            }
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.buffer.is_empty() => {
            app.should_exit = true;
        }
        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            app.input.insert_char(c);
        }
        (KeyCode::Backspace, _) => app.input.backspace(),
        (KeyCode::Delete, _) => app.input.delete_forward(),
        (KeyCode::Left, _) => app.input.cursor_left(),
        (KeyCode::Right, _) => app.input.cursor_right(),
        (KeyCode::Home, _) => app.input.cursor_home(),
        (KeyCode::End, _) => app.input.cursor_end(),
        (KeyCode::Up, _) => app.input.history_up(),
        (KeyCode::Down, _) => app.input.history_down(),
        (KeyCode::PageUp, _) => {
            if app.auto_scroll {
                app.scroll = app.last_max_scroll;
                app.auto_scroll = false;
            }
            app.scroll = app.scroll.saturating_sub(10);
        }
        (KeyCode::PageDown, _) => {
            let next = app.scroll.saturating_add(10);
            if next >= app.last_max_scroll {
                app.scroll = app.last_max_scroll;
                app.auto_scroll = true;
            } else {
                app.scroll = next;
            }
        }
        (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) || m.contains(KeyModifiers::ALT) => {
            app.input.insert_newline();
        }
        (KeyCode::Enter, _) => {
            let prompt = app.input.buffer.trim().to_string();
            if prompt.is_empty() {
                return;
            }
            // Ignore submit if a turn is already running.
            if app.running.is_some() {
                return;
            }

            let line = app.input.submit();
            app.auto_scroll = true;

            if prompt.starts_with('/') {
                handle_slash_command(&prompt, app);
                return;
            }

            // Fire UserPromptSubmit (best-effort, sync over the current
            // runtime). Hooks may rewrite the prompt via `UpdatedInput`.
            let prompt_for_hook = prompt.clone();
            let cwd_for_hook = app.cwd.clone();
            let hooks = app.agent.hooks();
            let hook_decision = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let ctx = caliban_agent_core::PromptCtx {
                        session_id: "tui",
                        cwd: &cwd_for_hook,
                        turn_index: 0,
                        prompt: &prompt_for_hook,
                        attachments: &[],
                    };
                    hooks.user_prompt_submit(&ctx).await
                })
            });
            let prompt = match hook_decision {
                Ok(caliban_agent_core::HookDecision::Allow) => prompt,
                Ok(caliban_agent_core::HookDecision::Deny(msg)) => {
                    app.toast = Some(toast::Toast::error(format!(
                        "prompt rejected by hook: {msg}"
                    )));
                    app.input.buffer = line;
                    app.input.cursor = app.input.buffer.len();
                    return;
                }
                Ok(caliban_agent_core::HookDecision::UpdatedInput(v)) => match v.as_str() {
                    Some(s) => s.to_string(),
                    None => prompt,
                },
                Err(e) => {
                    tracing::warn!(error = %e, "user_prompt_submit hook error (non-fatal)");
                    prompt
                }
            };

            // Resolve any @-attachments before we send. On failure, restore
            // the buffer and surface the error as a toast — no roundtrip.
            let workspace_root = app
                .args
                .workspace
                .clone()
                .unwrap_or_else(|| app.cwd.clone());
            let resolved = match attach::resolve_attachments(
                &line,
                &workspace_root,
                &app.cwd,
                app.args.max_attach_bytes,
                app.args.attach_budget_bytes,
            ) {
                Ok(r) => r,
                Err(e) => {
                    let hint = match &e {
                        attach::AttachError::Oversize { .. } => {
                            "Drop the @ or raise --max-attach-bytes."
                        }
                        attach::AttachError::BudgetExceeded { .. } => {
                            "Remove an attachment or raise --attach-budget-bytes."
                        }
                        attach::AttachError::NotUtf8 { .. } => {
                            "Binary files can't be inlined; ask me to Read it instead."
                        }
                        attach::AttachError::Io { .. } => "Check the path and try again.",
                    };
                    app.toast = Some(toast::Toast::error(format!("{e} — {hint}")));
                    app.input.buffer = line;
                    app.input.cursor = app.input.buffer.len();
                    return;
                }
            };
            let outgoing_text = attach::format_outgoing(&resolved);

            app.transcript
                .push(TranscriptLine::UserPrompt(prompt.clone()));
            for a in &resolved.attachments {
                app.transcript.push(TranscriptLine::Attached {
                    display_path: a.display_path.clone(),
                    bytes: a.bytes,
                });
            }

            // Build message history: in-memory history + new user prompt.
            let mut messages: Vec<caliban_provider::Message> = app.messages.clone();

            // Snapshot the current todos and rebuild message[0] so the model
            // sees the up-to-date task list at the start of every turn.
            let todo_snapshot = app.todos.lock().expect("todos lock poisoned").clone();
            if let Some(ref sp) = app.system_prompt {
                let with_todos = crate::system_prompt::append_todo_block(sp, &todo_snapshot);
                let has_system = messages
                    .first()
                    .is_some_and(|m| m.role == caliban_provider::Role::System);
                if has_system {
                    messages[0] = caliban_provider::Message::system_text(with_todos);
                } else {
                    messages.insert(0, caliban_provider::Message::system_text(with_todos));
                }
            }

            messages.push(caliban_provider::Message::user_text(outgoing_text));

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

    // After buffer mutations, re-evaluate menu state. `maybe_open_slash_menu`
    // is a no-op unless the buffer is exactly "/"; `refilter_slash_menu` is a
    // no-op unless the menu is currently open. `refresh_at_menu` opens or
    // refilters the @-completion menu based on whether the cursor sits in
    // an active @-token.
    app.input.maybe_open_slash_menu(SLASH_COMMANDS);
    app.input.refilter_slash_menu(SLASH_COMMANDS);
    refresh_at_menu(app);
}

/// Open or refilter the @-completion menu, given the current buffer state.
/// Closes the menu if the cursor no longer sits inside an @-token.
fn refresh_at_menu(app: &mut App) {
    use crate::tui::attach::{read_dir_candidates, split_at_token};
    use crate::tui::completer::{Candidate, rank};

    let Some((start, token)) = app.input.active_at_token() else {
        if matches!(app.input.mode, InputMode::AtMenu(_)) {
            app.input.close_menu();
        }
        return;
    };
    let cwd = app.cwd.clone();
    let workspace_root = app.args.workspace.clone().unwrap_or_else(|| cwd.clone());
    let home = dirs::home_dir();
    let (dir, name) = split_at_token(&token, &workspace_root, &cwd, home.as_deref());
    let show_hidden = name.starts_with('.');
    let raw = read_dir_candidates(&dir, show_hidden);
    let items: Vec<(&str, &str)> = raw
        .iter()
        .map(|c| (c.display.as_str(), c.insert.as_str()))
        .collect();
    let ranked = rank(&items, &name, 32);

    // The candidate `insert` from `read_dir_candidates` is just the leaf
    // name. Replacement spans @-trigger to end-of-token, so insert text
    // must reproduce the FULL new token including '@' and the directory
    // prefix the user already typed.
    let dir_prefix = &token[..token.len() - name.len()];
    let ranked_with_full_insert: Vec<Candidate> = ranked
        .into_iter()
        .map(|mut c| {
            c.insert = format!("@{dir_prefix}{}", c.insert);
            c
        })
        .collect();
    app.input.open_at_menu(start, ranked_with_full_insert);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_suffix_omitted_when_no_cache() {
        assert_eq!(format_cache_suffix(None, None), "");
        assert_eq!(format_cache_suffix(Some(0), Some(0)), "");
        assert_eq!(format_cache_suffix(Some(0), None), "");
        assert_eq!(format_cache_suffix(None, Some(0)), "");
    }

    #[test]
    fn cache_suffix_read_only() {
        assert_eq!(format_cache_suffix(Some(42), None), " (42 cached)");
        assert_eq!(format_cache_suffix(Some(42), Some(0)), " (42 cached)");
    }

    #[test]
    fn cache_suffix_write_only() {
        assert_eq!(format_cache_suffix(None, Some(100)), " (100 cache write)");
        assert_eq!(
            format_cache_suffix(Some(0), Some(100)),
            " (100 cache write)"
        );
    }

    #[test]
    fn cache_suffix_both() {
        assert_eq!(
            format_cache_suffix(Some(42), Some(100)),
            " (42 cached, 100 write)"
        );
    }

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
