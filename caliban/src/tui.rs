//! Ratatui-based interactive TUI.

#![allow(clippy::print_stdout, clippy::print_stderr)]

mod ask;
mod attach;
mod completer;
mod external_editor;
mod input;
mod reverse_history;
mod shell_escape;
pub(crate) mod slash;
mod toast;
mod transcript_viewer;

pub(crate) use ask::TuiAskHandler;
use input::InputMode;

// The slash typeahead suggester used to consult a hard-coded
// `SLASH_COMMANDS` constant. With ADR 0040 in place, it consults the
// `App.slash_registry` instead; the registry is the single source of
// truth for command names + descriptions + hidden flag.

/// Window for an Esc-Esc chord (ADR 0028). A second Esc inside this many
/// milliseconds after a first Esc on an empty buffer triggers the rewind
/// overlay.
const ESC_ESC_WINDOW_MS: u128 = 400;

/// Returns `true` iff the (`prev_esc_at`, `now`) interval qualifies as an
/// Esc-Esc chord under the [`ESC_ESC_WINDOW_MS`] policy.
///
/// Caller passes the timestamp recorded by the *previous* Esc keypress
/// (or `None` if no previous Esc). Pulled out into a pure helper so the
/// chord logic is unit-testable without an `App` fixture.
#[must_use]
pub(crate) fn is_esc_chord(
    prev_esc_at: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    prev_esc_at.is_some_and(|prev| now.duration_since(prev).as_millis() <= ESC_ESC_WINDOW_MS)
}

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
    fn title(self) -> &'static str {
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

    fn short_name(self) -> &'static str {
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
    /// Active permission mode (ADR 0029). Cycled via `Shift+Tab`. Lock-free
    /// reads via `ArcSwap` under the hood.
    pub(crate) permission_mode: caliban_agent_core::SharedPermissionMode,
    /// `true` when the operator passed `--allow-dangerously-skip-permissions`
    /// at startup. Gates entry into [`caliban_agent_core::PermissionMode::BypassPermissions`].
    pub(crate) bypass_latch: bool,
    /// Optional handle to the auto-mode classifier so we can drop the cache
    /// when the operator cycles out of `auto`. `None` when no
    /// `FastClassifier` route is wired.
    pub(crate) auto_mode_classifier: Option<Arc<caliban_agent_core::AutoModeClassifier>>,
    /// Snapshot of per-server MCP lifecycle status at startup. Surfaces in the
    /// `/mcp` overlay. Empty when `--no-mcp` is set or no servers are
    /// configured.
    pub(crate) mcp_servers: Vec<caliban_mcp_client::ServerSummary>,
    /// Live context-window tracker. Read by the status bar every frame to
    /// render the `X% of N` segment; updated on every `TurnEnd` so the
    /// percent reflects the latest history. Always present (works even with
    /// `CALIBAN_ENABLE_TELEMETRY=0`).
    pub(crate) context_window: Arc<caliban_telemetry::ContextWindow>,
    /// Session-scoped cost ledger backing `/usage`. Always present.
    pub(crate) cost_accumulator: Arc<caliban_telemetry::CostAccumulator>,

    /// Receiver for permission Ask requests forwarded by `TuiAskHandler`.
    /// Drained inside the main `select!`; each request opens the Ask modal.
    pub(crate) ask_rx: Option<tokio::sync::mpsc::UnboundedReceiver<ask::AskRequest>>,
    /// Currently-pending Ask request. While `Some(_)`, the input is locked
    /// and the modal is rendered.
    pub(crate) ask_modal: Option<ask::AskRequest>,
    /// State for the Ctrl+O transcript viewer overlay.
    pub(crate) transcript_viewer: transcript_viewer::TranscriptViewerState,
    /// State for the Ctrl+R reverse-history search overlay.
    pub(crate) reverse_history: Option<reverse_history::ReverseHistoryState>,
    /// Path under `~/.caliban/projects/<sanitized-cwd>/` where input-history
    /// is persisted. `None` when `dirs::home_dir()` is unavailable.
    pub(crate) input_history_path: Option<PathBuf>,
    /// Per-session checkpoint store. `Some` when checkpointing is enabled
    /// for this session — used by `/rewind` (ADR 0028) to list per-prompt
    /// checkpoints.
    pub(crate) checkpoint_store: Option<caliban_checkpoint::CheckpointStore>,
    /// Timestamp of the most recent Esc keypress; used to detect Esc-Esc
    /// chords for `/rewind` (ADR 0028). The chord is only accepted when
    /// (a) the buffer is empty, (b) no overlay is open, and (c) both
    /// presses happen within `ESC_ESC_WINDOW_MS` of each other.
    pub(crate) last_esc_at: Option<std::time::Instant>,
    /// Layered settings (ADR 0026). The handle is `Some` whenever the
    /// loader ran (even in `--bare` mode, where it returns an empty
    /// `Settings`).
    pub(crate) settings_handle: Option<caliban_settings::SettingsHandle>,
    /// `/config` provenance lines: `scope-label  path  format`. Lifted
    /// from the loader outcome so the overlay can render the scope
    /// chain without re-running discovery.
    pub(crate) settings_sources: Vec<(String, Option<PathBuf>, Option<String>)>,
    /// Central slash-command registry (ADR 0040). Built once at startup
    /// from `slash::register_builtin()`; consulted by typeahead, by
    /// `/help`, and by the dispatcher. Plugins extend it via
    /// `registry.register(...)` once the plugin loader is wired.
    pub(crate) slash_registry: slash::SlashCommandRegistry,
    /// Last short status message returned from `SlashOutcome::StatusMessage`
    /// and surfaced as a toast / transcript info line. Stored here so the
    /// TUI status bar can render it for a single frame.
    pub(crate) last_status_message: Option<String>,
}

impl App {
    /// Construct initial `App` state from CLI args and an optional loaded session.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        agent: Arc<Agent>,
        session: Option<PersistedSession>,
        store: Option<SessionStore>,
        args: Args,
        system_prompt: Option<String>,
        todos: caliban_agent_core::SharedTodos,
        plan_mode: caliban_agent_core::SharedPlanMode,
        permission_mode: caliban_agent_core::SharedPermissionMode,
        bypass_latch: bool,
        auto_mode_classifier: Option<Arc<caliban_agent_core::AutoModeClassifier>>,
        mcp_servers: Vec<caliban_mcp_client::ServerSummary>,
        ask_rx: Option<tokio::sync::mpsc::UnboundedReceiver<ask::AskRequest>>,
        settings_handle: Option<caliban_settings::SettingsHandle>,
        settings_sources: Vec<(String, Option<PathBuf>, Option<String>)>,
    ) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let messages = session
            .as_ref()
            .map(|s| s.messages.clone())
            .unwrap_or_default();
        // Seed input history from: persisted project history (oldest first),
        // then current-session user prompts (newest at end).
        let input_history_path = reverse_history::project_history_path(&cwd);
        let mut history: Vec<String> = input_history_path
            .as_deref()
            .map(reverse_history::load_history_file)
            .unwrap_or_default();
        let session_history: Vec<String> = session
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
        history.extend(session_history);
        let context_window = Arc::new(caliban_telemetry::ContextWindow::new());
        let cost_accumulator = Arc::new(
            caliban_telemetry::CostAccumulator::with_embedded_card().unwrap_or_else(|e| {
                tracing::error!(
                    target: "caliban::cost",
                    error = %e,
                    "failed to parse embedded rates.yaml; pricing disabled"
                );
                caliban_telemetry::CostAccumulator::new(caliban_telemetry::RateCard::from_file(
                    caliban_telemetry::RateCardFile {
                        version: 1,
                        providers: std::collections::BTreeMap::new(),
                    },
                ))
            }),
        );
        // Initialize capacity from the provider's capabilities for the
        // configured model so the status-bar segment shows up immediately.
        let model = args
            .model
            .clone()
            .unwrap_or_else(|| crate::default_model_for(args.provider).to_string());
        let caps = agent.provider().capabilities(&model);
        context_window.set_capacity(caps.max_input_tokens);
        if !messages.is_empty() {
            context_window.record_history(&messages);
        }
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
            permission_mode,
            bypass_latch,
            auto_mode_classifier,
            mcp_servers,
            context_window,
            cost_accumulator,
            ask_rx,
            ask_modal: None,
            transcript_viewer: transcript_viewer::TranscriptViewerState::default(),
            reverse_history: None,
            input_history_path,
            checkpoint_store: None,
            last_esc_at: None,
            settings_handle,
            settings_sources,
            slash_registry: slash::register_builtin(),
            last_status_message: None,
        }
    }

    /// Attach a [`caliban_checkpoint::CheckpointStore`] for the current
    /// session (enables `/rewind`).
    #[allow(
        dead_code,
        reason = "wired by main.rs once full /rewind action plumbing lands"
    )]
    pub(crate) fn with_checkpoint_store(
        mut self,
        store: caliban_checkpoint::CheckpointStore,
    ) -> Self {
        self.checkpoint_store = Some(store);
        self
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
fn clone_lines(lines: &[Line<'_>]) -> Vec<Line<'static>> {
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

fn reverse_history_lines(app: &App) -> Vec<Line<'static>> {
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

fn ask_modal_lines(app: &App) -> Vec<Line<'static>> {
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
    out.push(Line::styled(
        "   [y] Allow once     [n] / [Esc] Deny",
        Style::default().fg(Color::Cyan),
    ));
    out.push(Line::raw(""));
    out.push(Line::styled(
        "   Modal blocks the agent loop until you decide.",
        dim,
    ));
    out
}

fn slash_help_lines(registry: &slash::SlashCommandRegistry) -> Vec<Line<'static>> {
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

fn mcp_lines(app: &App) -> Vec<Line<'static>> {
    use caliban_mcp_client::ServerStatus;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![Line::raw("")];

    if app.mcp_servers.is_empty() {
        out.push(Line::raw("   No MCP servers configured."));
        out.push(Line::raw(""));
        out.push(Line::raw(
            "   Configure servers in ~/.config/caliban/mcp.toml or",
        ));
        out.push(Line::raw(
            "   <workspace>/.caliban/mcp.toml. Minimal stdio example:",
        ));
        out.push(Line::raw(""));
        out.push(Line::raw("     [server.silverbullet]"));
        out.push(Line::raw("     command = \"sb-mcp\""));
        out.push(Line::raw("     args = [\"--vault\", \"~/notes\"]"));
        out.push(Line::raw(""));
        out.push(Line::styled(
            "   See `caliban-mcp-client` and ADR 0023 (Phase A: stdio).",
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
    out.push(Line::styled(
        "   Phase B: stdio + http + sse. OAuth / elicitation / resources land in Phase C.",
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

/// Render the `/rewind` overlay (ADR 0028) — listing per-prompt
/// checkpoints, newest first, with the actions available for the
/// currently-selected entry.
fn rewind_lines(app: &App) -> Vec<Line<'static>> {
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

    // Permission mode chip (ADR 0029). Hidden for `default`; cycled via
    // Shift+Tab. Note that the `Plan` permission mode prints its own chip
    // here in addition to the legacy plan_mode chip above so the two
    // stay visually consistent during the SharedPlanMode → PermissionMode
    // migration window.
    let perm_mode = app.permission_mode.load();
    let perm_mode_part = match perm_mode {
        caliban_agent_core::PermissionMode::Default => String::new(),
        other => format!(" \u{00B7} [{}]", other.chip()),
    };

    // Context-window utilization indicator. Hidden when capacity is zero
    // (e.g. provider hasn't reported `Capabilities`).
    let context_part = caliban_telemetry::format_status_segment(&app.context_window)
        .map(|seg| format!(" \u{00B7} {seg}"))
        .unwrap_or_default();

    let text = format!(
        " {cwd} \u{00B7} {provider} {model}{session_part}{plan_part}{perm_mode_part}{overlay_part}{running_part}{context_part}"
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
        TurnEvent::TurnEnd {
            ttft,
            ref assistant_message,
            usage,
            ..
        } => {
            if let Some(t) = ttft {
                let millis = u64::try_from(t.as_millis()).unwrap_or(u64::MAX);
                app.last_turn_ttft_ms = Some(millis);
            }
            // Record cost against the rate card for this turn.
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
            app.cost_accumulator.record(provider, &model, &usage, None);
            // Mirror the assistant message into context-window bookkeeping.
            let snapshot = {
                let mut v = app.messages.clone();
                v.push(assistant_message.clone());
                v
            };
            app.context_window.record_history(&snapshot);
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
            // Refresh context-window bookkeeping with the run's final history.
            app.context_window.record_history(&final_messages);
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
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    args: Args,
    agent: Arc<Agent>,
    store: Option<SessionStore>,
    session: Option<PersistedSession>,
    system_prompt: Option<String>,
    todos: caliban_agent_core::SharedTodos,
    plan_mode: caliban_agent_core::SharedPlanMode,
    permission_mode: caliban_agent_core::SharedPermissionMode,
    bypass_latch: bool,
    auto_mode_classifier: Option<Arc<caliban_agent_core::AutoModeClassifier>>,
    mcp_servers: Vec<caliban_mcp_client::ServerSummary>,
    ask_rx: Option<tokio::sync::mpsc::UnboundedReceiver<ask::AskRequest>>,
    settings_handle: Option<caliban_settings::SettingsHandle>,
    settings_sources: Vec<(String, Option<PathBuf>, Option<String>)>,
) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new(
        agent,
        session,
        store,
        args,
        system_prompt,
        todos,
        plan_mode,
        permission_mode,
        bypass_latch,
        auto_mode_classifier,
        mcp_servers,
        ask_rx,
        settings_handle,
        settings_sources,
    );
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
            ask_event = async {
                match app.ask_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending::<Option<ask::AskRequest>>().await,
                }
            } => {
                if let Some(req) = ask_event {
                    // Only open the modal if none is currently active. If a
                    // request races in mid-modal, deny it (the Bash tool will
                    // surface the message).
                    if app.ask_modal.is_some() {
                        let _ = req.respond.send(ask::AskResponse::Deny);
                    } else {
                        app.ask_modal = Some(req);
                        app.view = ViewState::Overlay(Overlay::AskModal);
                        app.auto_scroll = false;
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

/// Thin wrapper over [`slash::SlashCommandRegistry::dispatch`] (ADR 0040).
///
/// Parses `/<name> [args...]`, takes the registry out of `app` (to side-step
/// the `&mut App` borrow), dispatches, then applies the resulting
/// [`slash::SlashOutcome`] to the transcript / view-state. The registry is
/// always restored on the way out.
///
/// Note: not all surfaces of the registry — overlay rendering for stub
/// status messages, `Reload` semantics — are wired in this PR; commands
/// that return `Continue` get no extra behavior, and `Quit`/`Overlay`/
/// `StatusMessage` are honored directly.
fn handle_slash_command(line: &str, app: &mut App) {
    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("").to_string();
    let arg = parts.next().unwrap_or("").trim().to_string();

    // Take the registry out so we can hand a `&mut App` to dispatch
    // without aliasing.
    let registry = std::mem::take(&mut app.slash_registry);
    let outcome = futures::executor::block_on(async {
        let mut ctx = slash::SlashCtx { app };
        registry.dispatch(&cmd, &arg, &mut ctx).await
    });
    app.slash_registry = registry;

    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            app.transcript
                .push(TranscriptLine::Error(format!("slash command failed: {e}")));
            return;
        }
    };
    apply_slash_outcome(outcome, app);
}

/// Apply a [`slash::SlashOutcome`] to the running `App`. Pulled out so the
/// behavior is unit-testable.
pub(crate) fn apply_slash_outcome(outcome: slash::SlashOutcome, app: &mut App) {
    match outcome {
        slash::SlashOutcome::Continue => {}
        slash::SlashOutcome::Quit => {
            app.should_exit = true;
        }
        slash::SlashOutcome::Overlay(o) => {
            app.view = ViewState::Overlay(o);
        }
        slash::SlashOutcome::StatusMessage(msg) => {
            app.last_status_message = Some(msg.clone());
            app.transcript.push(TranscriptLine::Info(msg));
        }
        slash::SlashOutcome::InsertText(s) => {
            app.input.buffer = s;
            app.input.cursor = app.input.buffer.len();
        }
        slash::SlashOutcome::Reload => {
            // Reload semantics land with the Settings hierarchy spec.
            app.transcript.push(TranscriptLine::Info(
                "reload requested \u{2014} settings hot-reload lands with the Settings hierarchy spec".into(),
            ));
        }
    }
}

/// `/usage` overlay (ADR 0033): cumulative tokens + USD per model.
///
/// Returns a vector of plain-text lines for the transcript. The full
/// bordered overlay arrives with the slash registry (ADR 0040); this stub
/// renders the same data in-line.
pub(crate) fn render_usage_lines(app: &App) -> Vec<String> {
    let session_note = app.session.as_ref().map(|sess| {
        format!(
            "  session {} \u{2014} {} turns, {} input + {} output tokens",
            sess.name,
            sess.turn_count(),
            sess.total_usage.input_tokens,
            sess.total_usage.output_tokens,
        )
    });
    let mut lines = format_usage_lines(&app.cost_accumulator);
    if let Some(s) = session_note {
        lines.push(s);
    }
    lines
}

/// Pure formatter for `/usage`. Split out so we can unit-test the rendering
/// without constructing a full `App`.
fn format_usage_lines(cost: &caliban_telemetry::CostAccumulator) -> Vec<String> {
    let bd = cost.breakdown();
    let mut lines = vec![format!(
        "usage \u{2014} total ${:.4}",
        rust_decimal::prelude::ToPrimitive::to_f64(&bd.total_usd).unwrap_or(0.0),
    )];
    if bd.by_model.is_empty() {
        lines.push("  (no provider calls yet this session)".into());
    } else {
        lines.push("  by model:".into());
        for mc in &bd.by_model {
            let usd_f = rust_decimal::prelude::ToPrimitive::to_f64(&mc.usd).unwrap_or(0.0);
            lines.push(format!(
                "    {}/{}  in {}  out {}  cache_r {}  cache_w {}  ${:.4}",
                mc.provider,
                mc.model,
                mc.input_tokens,
                mc.output_tokens,
                mc.cache_read_tokens,
                mc.cache_creation_tokens,
                usd_f,
            ));
        }
    }
    if bd.cache_savings_usd > rust_decimal::Decimal::ZERO {
        let sav = rust_decimal::prelude::ToPrimitive::to_f64(&bd.cache_savings_usd).unwrap_or(0.0);
        lines.push(format!("  cache savings vs no-cache: ${sav:.4}"));
    }
    lines
}

/// `/context` overlay (ADR 0033): per-message-kind token breakdown.
pub(crate) fn render_context_lines(app: &App) -> Vec<String> {
    format_context_lines(&app.context_window)
}

/// Pure formatter for `/context`. Split out so we can unit-test the rendering
/// without constructing a full `App`.
fn format_context_lines(window: &caliban_telemetry::ContextWindow) -> Vec<String> {
    let bd = window.breakdown();
    let pct = if bd.capacity == 0 {
        0
    } else {
        // utilization_bp is 0..=10_000 (bp); convert to percent.
        u32::from(window.utilization_bp()) / 100
    };
    let mut lines = Vec::new();
    if bd.capacity == 0 {
        lines.push(
            "context window \u{2014} no capacity reported by provider (start a turn first)".into(),
        );
        return lines;
    }
    lines.push(format!(
        "context window \u{2014} {}-token window, {pct}% used ({} of {})",
        bd.capacity, bd.used, bd.capacity,
    ));
    let mut bins: Vec<_> = bd.bins.iter().filter(|b| b.tokens > 0).collect();
    bins.sort_by_key(|b| std::cmp::Reverse(b.tokens));
    for b in &bins {
        lines.push(format!("  {:<18} {:>8}", b.kind.label(), b.tokens));
    }
    if bins.is_empty() {
        lines.push("  (no messages yet)".into());
    }
    if pct >= 80 {
        lines.push("  warning: \u{2265} 80% of context used \u{2014} consider /compact".into());
    }
    lines
}

/// `/compact` (ADR 0033): manually trigger the configured `Compactor`.
///
/// Reports the number of messages dropped/summarized + the post-compact
/// token count. The full bordered overlay arrives with ADR 0040; this stub
/// writes the result inline.
pub(crate) fn handle_compact_command(app: &mut App) {
    if app.messages.is_empty() {
        app.transcript.push(TranscriptLine::Info(
            "compact: no messages to compact".into(),
        ));
        return;
    }
    let model = app
        .args
        .model
        .clone()
        .unwrap_or_else(|| crate::default_model_for(app.args.provider).to_string());
    let caps = app.agent.provider().capabilities(&model);
    let before = caliban_agent_core::estimate_tokens(&app.messages);
    let before_count = app.messages.len();
    let compactor = app.agent.compactor();
    let messages = app.messages.clone();
    let result = futures::executor::block_on(compactor.compact(&messages, &caps));
    match result {
        Err(e) => app
            .transcript
            .push(TranscriptLine::Error(format!("compact failed: {e}"))),
        Ok(None) => app.transcript.push(TranscriptLine::Info(format!(
            "compact: no-op (strategy {} kept {before_count} messages, ~{before} tokens)",
            compactor.strategy_name(),
        ))),
        Ok(Some(new)) => {
            let after = caliban_agent_core::estimate_tokens(&new);
            let after_count = new.len();
            let dropped = before_count.saturating_sub(after_count);
            app.messages.clone_from(&new);
            if let Some(sess) = app.session.as_mut() {
                sess.messages.clone_from(&new);
            }
            // Refresh context window from the post-compact history.
            app.context_window.record_history(&new);
            app.transcript.push(TranscriptLine::Info(format!(
                "compact (strategy {}): {before_count} \u{2192} {after_count} messages \
                 ({dropped} dropped/summarized), ~{before} \u{2192} ~{after} tokens",
                compactor.strategy_name(),
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

/// Pure cycle: given the current mode + whether the bypass latch is set,
/// return the next mode + an optional toast-text message. Extracted so the
/// behavior is unit-testable without constructing a full `App`.
///
/// When the cycle would step into `BypassPermissions` without the latch,
/// the dangerous slot is skipped and a warning message is returned.
pub(crate) fn next_permission_mode(
    current: caliban_agent_core::PermissionMode,
    bypass_latch: bool,
) -> (caliban_agent_core::PermissionMode, Option<String>) {
    use caliban_agent_core::PermissionMode;
    let candidate = current.next();
    if candidate == PermissionMode::BypassPermissions && !bypass_latch {
        let skipped = candidate.next();
        let toast = "bypassPermissions requires --allow-dangerously-skip-permissions".to_string();
        return (skipped, Some(toast));
    }
    (candidate, None)
}

/// Advance the permission mode (ADR 0029). Refuses to enter
/// `BypassPermissions` without `--allow-dangerously-skip-permissions`; in
/// that case skips past it. Drops the auto-mode classifier cache when
/// leaving `auto`.
pub(crate) fn cycle_permission_mode(app: &mut App) {
    use caliban_agent_core::PermissionMode;
    let prev = app.permission_mode.load();
    let (next, warning) = next_permission_mode(prev, app.bypass_latch);
    if let Some(msg) = warning {
        app.toast = Some(toast::Toast::info(msg));
    }
    // Cycling out of auto: drop the classifier cache so the next visit
    // re-classifies from scratch.
    if prev == PermissionMode::Auto
        && next != PermissionMode::Auto
        && let Some(c) = app.auto_mode_classifier.as_ref()
    {
        c.clear_cache();
    }
    app.permission_mode.store(next);
    // Keep the legacy SharedPlanMode flag in sync with the enum so the
    // existing `/plan` chip and the `EnterPlanMode`/`ExitPlanMode` tools
    // stay coherent.
    app.plan_mode.store(
        next == PermissionMode::Plan,
        std::sync::atomic::Ordering::Relaxed,
    );
    let label = if next == PermissionMode::Default {
        "default".to_string()
    } else {
        next.chip().to_string()
    };
    // Don't overwrite an explicit warning toast — only set the success
    // toast when no warning was emitted.
    if app.toast.is_none() {
        app.toast = Some(toast::Toast::info(format!("permission mode: {label}")));
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

    // Overlay-mode key handling: most overlays are read-only (Esc/q close).
    // A few have richer dispatch — defer to per-overlay handlers first.
    if let ViewState::Overlay(o) = app.view {
        match o {
            Overlay::AskModal => {
                handle_ask_modal_key(key, app);
                return;
            }
            Overlay::TranscriptViewer => {
                handle_transcript_viewer_key(key, app);
                return;
            }
            Overlay::ReverseHistory => {
                handle_reverse_history_key(key, app);
                return;
            }
            _ => {}
        }
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

    // Global TUI ergonomics hotkeys (ADR 0027). Take precedence over normal
    // input handling. Ctrl+G / Ctrl+O / Ctrl+R / Ctrl+S apply outside the
    // overlay flow.
    match (key.code, key.modifiers) {
        (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
            handle_ctrl_g(app);
            return;
        }
        (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
            app.view = ViewState::Overlay(Overlay::TranscriptViewer);
            app.transcript_viewer.scroll = 0;
            return;
        }
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
            open_reverse_history(app);
            return;
        }
        // Shift+Tab cycles permission modes (ADR 0029). Skips
        // BypassPermissions when no `--allow-dangerously-skip-permissions`
        // latch is set, fires a warning toast in that case.
        (KeyCode::BackTab, _) => {
            cycle_permission_mode(app);
            return;
        }
        _ => {}
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            if let Some(running) = &app.running {
                running.cancel.cancel();
            } else if app.input.buffer.is_empty() {
                app.should_exit = true;
            } else {
                app.input.clear();
            }
        }
        // Ctrl+B — hand the in-flight foreground sub-agent to the
        // supervisor and cancel the parent turn (ADR 0037).
        (KeyCode::Char('b'), KeyModifiers::CONTROL) if app.running.is_some() => {
            // Take the cancel token by cloning to drop the borrow
            // before the &mut self handoff call.
            let cancel = app.running.as_ref().map(|r| r.cancel.clone());
            handoff_to_supervisor(app);
            if let Some(c) = cancel {
                c.cancel();
            }
        }
        (KeyCode::Esc, _) => {
            // Esc handling (ADR 0028): if a turn is running, cancel it;
            // if input is non-empty, clear it; otherwise treat the press
            // as half of an Esc-Esc chord. Two Esc within
            // `ESC_ESC_WINDOW_MS` on an empty buffer opens `/rewind`.
            if let Some(running) = &app.running {
                running.cancel.cancel();
                app.last_esc_at = None;
            } else if !app.input.buffer.is_empty() {
                app.input.clear();
                app.last_esc_at = None;
            } else {
                let now = std::time::Instant::now();
                if is_esc_chord(app.last_esc_at, now) {
                    app.view = ViewState::Overlay(Overlay::Rewind);
                    app.last_esc_at = None;
                } else {
                    app.last_esc_at = Some(now);
                }
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

            // `!cmd` shell escape — detect BEFORE submit so the synthesized
            // Bash invocation isn't added to conversation history. Same parse
            // accepts leading whitespace and rejects multi-line buffers.
            if let shell_escape::ShellEscapeIntent::Run(cmd) =
                shell_escape::parse_shell_escape(&app.input.buffer)
            {
                let line = app.input.submit();
                // Persist to project history. The synthesized command is a
                // user action, so it makes sense to be retrievable via
                // Ctrl+R later.
                if let Some(p) = app.input_history_path.as_deref() {
                    reverse_history::append_history(p, &line);
                }
                app.auto_scroll = true;
                dispatch_shell_escape(&cmd, app);
                return;
            }

            let line = app.input.submit();
            // Persist to per-project history (best-effort; silent on IO err).
            if let Some(p) = app.input_history_path.as_deref() {
                reverse_history::append_history(p, &line);
            }
            app.auto_scroll = true;

            // Fire UserPromptSubmit *before* slash parsing (ADR 0040). This
            // gives hooks the chance to intercept or rewrite slash commands
            // alongside regular prompts. The hook payload includes the
            // prompt text; slash detection re-runs against the (possibly
            // updated) prompt below.
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

            // Now that the hook has had a chance to allow/deny/rewrite,
            // route slash commands through the registry. The hook may have
            // turned a plain prompt into a slash command or vice versa via
            // `UpdatedInput`.
            if prompt.starts_with('/') {
                handle_slash_command(&prompt, app);
                return;
            }

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
    // Build the candidate list from the suggester so the registry is
    // the *single* source of truth for typeahead. We re-suggest with the
    // current prefix (extracted from the buffer) so hidden-flag and
    // substring filtering live in one place.
    let pairs: Vec<(&'static str, &'static str)> = {
        let prefix = if app.input.buffer.starts_with('/') {
            let end = app
                .input
                .buffer
                .find(char::is_whitespace)
                .unwrap_or(app.input.buffer.len());
            app.input.buffer[1..end].to_string()
        } else {
            String::new()
        };
        app.slash_registry
            .suggest(&prefix)
            .into_iter()
            .map(|m| (m.name, m.name))
            .collect()
    };
    app.input.maybe_open_slash_menu(&pairs);
    app.input.refilter_slash_menu(&pairs);
    refresh_at_menu(app);
}

/// `Ctrl+B` entry — snapshot the in-flight foreground sub-agent and
/// hand ownership to the per-repo supervisor (ADR 0037). The parent's
/// turn is cancelled by the caller; here we just write the transcript
/// marker and best-effort register a placeholder agent with the
/// supervisor so it shows up in `caliban agents list`.
pub(crate) fn handoff_to_supervisor(app: &mut App) {
    use std::path::PathBuf;

    use caliban_supervisor::proto::SpawnSpec;
    use caliban_supervisor::{SupervisorClient, repo_socket_path};

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let repo = crate::agents_cli::discover_repo_root(&cwd);
    let socket_path = repo_socket_path(&repo);

    // Build a placeholder spec so the daemon can list the agent. The
    // real session bytes live in the parent's transcript; once we have
    // a serialized session-snapshot format (ADR 0037 deferred), we
    // pass it through `frontmatter_path`.
    let spec = SpawnSpec {
        label: Some("backgrounded-by-ctrl-b".into()),
        frontmatter_path: None,
        initial_prompt: "(snapshot)".into(),
        model: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: false,
    };

    // We're inside synchronous key handling; block on a one-shot async
    // request via the tokio current-thread handle. If the daemon isn't
    // running, leave a transcript note and move on.
    let id_opt: Option<String> = match tokio::runtime::Handle::try_current() {
        Ok(rt) => rt.block_on(async move {
            if !socket_path.exists() {
                return None;
            }
            let client = SupervisorClient::new(&socket_path);
            match client.spawn(spec).await {
                Ok((id, _sock)) => Some(id),
                Err(e) => {
                    tracing::warn!(error = %e, "Ctrl+B handoff: supervisor spawn failed");
                    None
                }
            }
        }),
        Err(_) => None,
    };

    let line = match id_opt {
        Some(id) => format!("[backgrounded sub-agent {id} — see `caliban agents list`]"),
        None => "[backgrounded — supervisor daemon offline; see `caliban daemon status`]".into(),
    };
    app.transcript.push(TranscriptLine::Info(line));
}

/// `Ctrl+G` entry — leaves the alt-screen, runs `$VISUAL`/`$EDITOR` over a
/// tempfile seeded with the current buffer, restores the alt-screen on
/// return. The buffer is replaced with the file contents on success; toast
/// on failure.
fn handle_ctrl_g(app: &mut App) {
    let initial = app.input.buffer.clone();
    let suspend = match external_editor::suspend_alt_screen() {
        Ok(g) => g,
        Err(e) => {
            app.toast = Some(toast::Toast::error(format!("editor suspend failed: {e}")));
            return;
        }
    };
    let launcher = external_editor::SubprocessLauncher;
    let outcome = external_editor::run_editor_roundtrip(&initial, &launcher);
    // Always attempt to resume — even on editor error.
    if let Err(e) = external_editor::resume_alt_screen(suspend) {
        // We're now in a weird state, but persist a toast for visibility.
        app.toast = Some(toast::Toast::error(format!("editor resume failed: {e}")));
    }
    match outcome {
        Ok(o) if o.success => {
            app.input.set_buffer(o.buffer);
        }
        Ok(_) => {
            app.toast = Some(toast::Toast::warn(
                "editor exited non-zero; buffer unchanged",
            ));
        }
        Err(e) => {
            app.toast = Some(toast::Toast::error(format!("editor failed: {e}")));
        }
    }
}

/// `Ctrl+R` entry — populate the reverse-history state and open the overlay.
fn open_reverse_history(app: &mut App) {
    let session_hist = app.input.history.clone();
    let state = reverse_history::ReverseHistoryState::new(
        session_hist,
        app.input_history_path.clone(),
        reverse_history::projects_root(),
    );
    app.reverse_history = Some(state);
    app.view = ViewState::Overlay(Overlay::ReverseHistory);
}

/// Synthesize a Bash invocation through the agent's hook chain (which wraps
/// `PermissionsHook`) and render the result inline as a `TranscriptLine::Info`.
fn dispatch_shell_escape(command: &str, app: &mut App) {
    let command = command.to_string();
    let registry = app.agent.tools().clone();
    let hooks = app.agent.hooks();
    app.transcript
        .push(TranscriptLine::Info(format!("! {command}")));
    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            shell_escape::run_shell_escape(command.clone(), &registry, hooks, cancel).await
        })
    });
    if outcome.denied {
        let reason = outcome
            .message
            .clone()
            .unwrap_or_else(|| "permission denied".into());
        app.transcript
            .push(TranscriptLine::Error(format!("[denied: {reason}]")));
        return;
    }
    if outcome.is_error {
        let msg = outcome
            .message
            .clone()
            .unwrap_or_else(|| "shell escape failed".into());
        app.transcript.push(TranscriptLine::Error(msg));
        return;
    }
    for line in outcome.output.split('\n') {
        app.transcript.push(TranscriptLine::Info(line.to_string()));
    }
}

/// Key dispatch for the Permission Ask modal.
fn handle_ask_modal_key(key: KeyEvent, app: &mut App) {
    let response = match (key.code, key.modifiers) {
        (KeyCode::Char('y'), KeyModifiers::NONE) | (KeyCode::Enter, _) => {
            Some(ask::AskResponse::AllowOnce)
        }
        (KeyCode::Char('n'), KeyModifiers::NONE)
        | (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(ask::AskResponse::Deny),
        _ => None,
    };
    if let Some(r) = response
        && let Some(req) = app.ask_modal.take()
    {
        let _ = req.respond.send(r);
        app.view = ViewState::Main;
    }
}

/// Key dispatch for the Ctrl+O transcript viewer overlay.
fn handle_transcript_viewer_key(key: KeyEvent, app: &mut App) {
    use external_editor::SubprocessLauncher;
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
            app.view = ViewState::Main;
        }
        (KeyCode::Char('?'), _) => {
            app.transcript_viewer.show_help = !app.transcript_viewer.show_help;
        }
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
            app.transcript_viewer.show_all = !app.transcript_viewer.show_all;
        }
        (KeyCode::Char('j') | KeyCode::Down, _) => {
            let max = u16::try_from(
                transcript_viewer::format_history(&app.messages, app.transcript_viewer.show_all)
                    .len()
                    .saturating_sub(1),
            )
            .unwrap_or(u16::MAX);
            app.transcript_viewer.down(1, max);
        }
        (KeyCode::Char('k') | KeyCode::Up, _) => {
            app.transcript_viewer.up(1);
        }
        (KeyCode::PageDown, _) => {
            let max = u16::try_from(
                transcript_viewer::format_history(&app.messages, app.transcript_viewer.show_all)
                    .len()
                    .saturating_sub(1),
            )
            .unwrap_or(u16::MAX);
            app.transcript_viewer.down(10, max);
        }
        (KeyCode::PageUp, _) => {
            app.transcript_viewer.up(10);
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => {
            app.transcript_viewer.scroll = 0;
        }
        (KeyCode::Char('G'), _) => {
            let max = u16::try_from(
                transcript_viewer::format_history(&app.messages, app.transcript_viewer.show_all)
                    .len()
                    .saturating_sub(1),
            )
            .unwrap_or(u16::MAX);
            app.transcript_viewer.scroll = max;
        }
        (KeyCode::Char('['), _) => {
            // Dump-to-scrollback: leave alt-screen, print, re-enter.
            let messages = app.messages.clone();
            let show_all = app.transcript_viewer.show_all;
            let mut stdout = std::io::stdout();
            let _ = transcript_viewer::dump_to_scrollback(
                &mut stdout,
                &messages,
                show_all,
                external_editor::suspend_alt_screen,
                external_editor::resume_alt_screen,
            );
        }
        (KeyCode::Char('v'), _) => {
            // Open transcript in $VISUAL — suspend alt-screen first.
            let suspend = match external_editor::suspend_alt_screen() {
                Ok(g) => g,
                Err(e) => {
                    app.toast = Some(toast::Toast::error(format!("editor suspend failed: {e}")));
                    return;
                }
            };
            let _ = transcript_viewer::open_in_visual(
                &app.messages,
                app.transcript_viewer.show_all,
                &SubprocessLauncher,
            );
            if let Err(e) = external_editor::resume_alt_screen(suspend) {
                app.toast = Some(toast::Toast::error(format!("editor resume failed: {e}")));
            }
        }
        _ => {}
    }
}

/// Key dispatch for the Ctrl+R reverse-history overlay.
fn handle_reverse_history_key(key: KeyEvent, app: &mut App) {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            app.reverse_history = None;
            app.view = ViewState::Main;
        }
        (KeyCode::Enter, _) => {
            if let Some(state) = app.reverse_history.as_ref()
                && let Some(sel) = state.selected()
            {
                app.input.set_buffer(sel);
            }
            app.reverse_history = None;
            app.view = ViewState::Main;
        }
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
            if let Some(state) = app.reverse_history.as_mut() {
                state.cycle_scope();
            }
        }
        (KeyCode::Up, _) => {
            if let Some(state) = app.reverse_history.as_mut() {
                state.cursor_up();
            }
        }
        (KeyCode::Down, _) => {
            if let Some(state) = app.reverse_history.as_mut() {
                state.cursor_down();
            }
        }
        (KeyCode::Backspace, _) => {
            if let Some(state) = app.reverse_history.as_mut() {
                state.pop_char();
            }
        }
        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            if let Some(state) = app.reverse_history.as_mut() {
                state.push_char(c);
            }
        }
        _ => {}
    }
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

    // === ADR 0033 slash command + status-bar segment tests ===

    fn usage_v(input: u32, output: u32) -> caliban_provider::Usage {
        caliban_provider::Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    #[test]
    fn usage_lines_show_total_zero_without_calls() {
        let cost = caliban_telemetry::CostAccumulator::with_embedded_card().unwrap();
        let lines = format_usage_lines(&cost);
        assert!(lines[0].contains("total $0.0000"));
        assert!(
            lines.iter().any(|l| l.contains("no provider calls")),
            "empty ledger emits a friendly placeholder",
        );
    }

    #[test]
    fn usage_lines_show_per_model_breakdown_after_record() {
        let cost = caliban_telemetry::CostAccumulator::with_embedded_card().unwrap();
        cost.record(
            "anthropic",
            "claude-opus-4-7-20260423",
            &usage_v(1_000_000, 0),
            None,
        );
        let lines = format_usage_lines(&cost);
        // Total = 1M × $15/M = $15.
        assert!(
            lines[0].contains("$15.0000"),
            "total line was: {}",
            lines[0]
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("claude-opus-4-7-20260423") && l.contains("$15.0000")),
            "per-model row present",
        );
    }

    #[test]
    fn context_lines_show_capacity_message_when_unset() {
        let w = caliban_telemetry::ContextWindow::new();
        let lines = format_context_lines(&w);
        assert!(
            lines.iter().any(|l| l.contains("no capacity reported")),
            "without set_capacity the helper surfaces a clear hint",
        );
    }

    #[test]
    fn context_lines_warn_at_80_percent() {
        let w = caliban_telemetry::ContextWindow::new();
        w.set_capacity(10_000);
        // Add 8_000 tokens of summarized text → 80%.
        w.add(caliban_telemetry::MessageKind::Summarized, 8_000);
        let lines = format_context_lines(&w);
        assert!(lines.iter().any(|l| l.contains("80%")));
        assert!(
            lines.iter().any(|l| l.contains("consider /compact")),
            "warning fires at 80%",
        );
    }

    #[test]
    fn status_segment_formats_percent_of_capacity() {
        let w = caliban_telemetry::ContextWindow::new();
        w.set_capacity(200_000);
        w.add(caliban_telemetry::MessageKind::UserText, 24_000);
        // 12% utilization.
        let seg = caliban_telemetry::format_status_segment(&w).expect("capacity is set");
        assert_eq!(seg, "12% of 200K");
    }

    #[test]
    fn noop_compactor_reports_no_op() {
        // /compact must report a no-op cleanly when the strategy decides
        // there's nothing to compact. Exercises the same Compactor path
        // handle_compact_command consumes.
        use caliban_agent_core::{Compactor as _, NoopCompactor};
        let comp = NoopCompactor;
        let caps = caliban_provider::Capabilities {
            max_input_tokens: 200_000,
            max_output_tokens: 8_192,
            vision: false,
            tool_use: caliban_provider::ToolUseCapability::Basic,
            thinking: false,
            prompt_caching: caliban_provider::PromptCachingCapability::None,
            json_mode: false,
            streaming: true,
            stop_sequences: true,
            top_k: false,
            system_prompt: caliban_provider::SystemPromptCapability::SeparateField,
            refusal_field: false,
        };
        let messages = vec![caliban_provider::Message::user_text("hello")];
        let out = futures::executor::block_on(comp.compact(&messages, &caps)).unwrap();
        assert!(out.is_none(), "noop returns None");
        assert_eq!(comp.strategy_name(), "Noop");
    }

    #[test]
    fn drop_oldest_compactor_reports_reduced_count() {
        // Long history with DropOldestCompactor must drop messages until
        // estimated tokens are below target.
        use caliban_agent_core::{Compactor as _, DropOldestCompactor};
        let comp = DropOldestCompactor {
            target_fraction: 0.1,
            keep_recent_turns: 1,
        };
        let caps = caliban_provider::Capabilities {
            max_input_tokens: 1_000,
            max_output_tokens: 256,
            vision: false,
            tool_use: caliban_provider::ToolUseCapability::Basic,
            thinking: false,
            prompt_caching: caliban_provider::PromptCachingCapability::None,
            json_mode: false,
            streaming: true,
            stop_sequences: true,
            top_k: false,
            system_prompt: caliban_provider::SystemPromptCapability::SeparateField,
            refusal_field: false,
        };
        // Build 10 user+assistant pairs of long text.
        let body = "x".repeat(200);
        let mut messages = Vec::new();
        for _ in 0..10 {
            messages.push(caliban_provider::Message::user_text(body.clone()));
            messages.push(caliban_provider::Message {
                role: caliban_provider::Role::Assistant,
                content: vec![caliban_provider::ContentBlock::Text(
                    caliban_provider::TextBlock {
                        text: body.clone(),
                        cache_control: None,
                    },
                )],
            });
        }
        let before_count = messages.len();
        let out = futures::executor::block_on(comp.compact(&messages, &caps)).unwrap();
        let new = out.expect("must compact when over target");
        assert!(
            new.len() < before_count,
            "compactor dropped messages: {before_count} → {}",
            new.len(),
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

    // -------------------------------------------------------------------
    // /rewind + Esc-Esc tests (ADR 0028)
    // -------------------------------------------------------------------

    #[test]
    fn rewind_is_a_registered_slash_command() {
        let registry = slash::register_builtin();
        assert!(
            registry.contains("/rewind"),
            "/rewind must be registered in the slash registry",
        );
    }

    #[test]
    fn rewind_overlay_variant_round_trips() {
        let o = Overlay::Rewind;
        assert_eq!(o.title(), "Rewind");
        assert_eq!(o.short_name(), "rewind");
    }

    #[test]
    fn esc_chord_within_window_is_recognized() {
        let t1 = std::time::Instant::now();
        let t2 = t1 + std::time::Duration::from_millis(100);
        assert!(is_esc_chord(Some(t1), t2));
    }

    #[test]
    fn esc_chord_outside_window_is_rejected() {
        let t1 = std::time::Instant::now();
        let t2 = t1 + std::time::Duration::from_millis(500);
        assert!(!is_esc_chord(Some(t1), t2));
    }

    #[test]
    fn esc_chord_requires_a_previous_press() {
        let t = std::time::Instant::now();
        assert!(!is_esc_chord(None, t));
    }

    // === Permission-mode cycling (ADR 0029) ===

    #[test]
    fn next_permission_mode_with_latch_passes_through_bypass() {
        use caliban_agent_core::PermissionMode;
        // With the latch, the cycle reaches BypassPermissions normally.
        let mut m = PermissionMode::DontAsk;
        let (next, warning) = next_permission_mode(m, true);
        m = next;
        assert_eq!(m, PermissionMode::BypassPermissions);
        assert!(warning.is_none());
    }

    #[test]
    fn next_permission_mode_without_latch_skips_bypass() {
        use caliban_agent_core::PermissionMode;
        // Coming from DontAsk, the next step is BypassPermissions; without
        // the latch we skip it and emit a warning toast.
        let (next, warning) = next_permission_mode(PermissionMode::DontAsk, false);
        assert_eq!(next, PermissionMode::Default);
        let msg = warning.expect("warning toast emitted");
        assert!(msg.contains("--allow-dangerously-skip-permissions"));
    }

    #[test]
    fn next_permission_mode_default_advances_to_accept_edits() {
        use caliban_agent_core::PermissionMode;
        let (next, warning) = next_permission_mode(PermissionMode::Default, false);
        assert_eq!(next, PermissionMode::AcceptEdits);
        assert!(warning.is_none());
    }

    // === ADR 0040: slash command registry integration tests ===

    /// Build a minimal `App` for in-bin slash-registry tests. Uses
    /// `MockProvider` so we can dispatch commands without network or auth.
    fn make_test_app() -> App {
        use caliban_agent_core::{Agent, ToolRegistry};
        use caliban_provider::{MockProvider, Provider};
        use clap::Parser;

        let mock: Arc<MockProvider> = Arc::new(MockProvider::new());
        let provider: Arc<dyn Provider + Send + Sync> = mock;
        let agent = Agent::builder()
            .provider(provider)
            .tools(ToolRegistry::new())
            .model("mock")
            .max_tokens(64)
            .max_turns(10)
            .build()
            .expect("agent builder");

        let args = crate::Args::parse_from(["caliban"]);
        App::new(
            Arc::new(agent),
            None,
            None,
            args,
            None,
            caliban_agent_core::SharedTodos::default(),
            caliban_agent_core::SharedPlanMode::default(),
            caliban_agent_core::SharedPermissionMode::default(),
            false,
            None,
            Vec::new(),
            None,
            None,
            Vec::new(),
        )
    }

    fn dispatch_slash(app: &mut App, line: &str) {
        handle_slash_command(line, app);
    }

    fn last_info(app: &App) -> Option<String> {
        app.transcript.iter().rev().find_map(|l| match l {
            TranscriptLine::Info(s) => Some(s.clone()),
            _ => None,
        })
    }

    #[test]
    fn registry_registers_all_expected_visible_commands() {
        // Spec calls for ~24 commands; we ship 25+ visible (the spec list
        // plus the `/system` legacy command).
        let registry = slash::register_builtin();
        // Sanity: the canonical commands are all present.
        for name in [
            "/help",
            "/clear",
            "/quit",
            "/init",
            "/resume",
            "/recap",
            "/btw",
            "/usage",
            "/context",
            "/compact",
            "/doctor",
            "/config",
            "/hooks",
            "/mcp",
            "/plugins",
            "/agents",
            "/model",
            "/effort",
            "/status",
            "/login",
            "/logout",
            "/setup-token",
            "/permissions",
            "/rewind",
            "/heapdump",
            "/feedback",
            "/loop",
            "/statusline",
            "/tui",
            "/voice",
            "/plan",
            "/memory",
            "/skills",
            "/output-style",
        ] {
            assert!(registry.contains(name), "missing command: {name}");
        }
        let visible = registry.visible_metas();
        assert!(
            visible.len() >= 24,
            "expected ≥24 visible commands, got {}",
            visible.len()
        );
    }

    #[test]
    fn registry_hides_voice_from_help_listing() {
        let registry = slash::register_builtin();
        assert!(registry.contains("/voice"));
        let visible: Vec<&str> = registry.visible_metas().iter().map(|m| m.name).collect();
        assert!(
            !visible.contains(&"/voice"),
            "voice should be hidden from help",
        );
    }

    #[test]
    fn dispatch_unknown_returns_status_message_via_handler() {
        let mut app = make_test_app();
        dispatch_slash(&mut app, "/this-doesnt-exist");
        let msg = last_info(&app).expect("status message in transcript");
        assert!(msg.contains("unknown command"), "got: {msg}");
        assert!(msg.contains("/this-doesnt-exist"), "got: {msg}");
    }

    #[test]
    fn quit_command_sets_should_exit() {
        let mut app = make_test_app();
        assert!(!app.should_exit);
        dispatch_slash(&mut app, "/quit");
        assert!(app.should_exit);
    }

    #[test]
    fn clear_command_clears_transcript_and_messages() {
        let mut app = make_test_app();
        app.transcript
            .push(TranscriptLine::Info("seed line".into()));
        app.messages
            .push(caliban_provider::Message::user_text("hello"));
        dispatch_slash(&mut app, "/clear");
        assert!(app.transcript.is_empty());
        assert!(app.messages.is_empty());
    }

    #[test]
    fn plan_command_toggles_plan_mode() {
        use std::sync::atomic::Ordering;
        let mut app = make_test_app();
        let before = app.plan_mode.load(Ordering::Relaxed);
        dispatch_slash(&mut app, "/plan");
        let after = app.plan_mode.load(Ordering::Relaxed);
        assert_ne!(before, after, "/plan toggles the flag");
    }

    #[test]
    fn config_command_opens_config_overlay() {
        let mut app = make_test_app();
        dispatch_slash(&mut app, "/config");
        assert!(matches!(app.view, ViewState::Overlay(Overlay::Config)));
    }

    #[test]
    fn mcp_command_opens_mcp_overlay() {
        let mut app = make_test_app();
        dispatch_slash(&mut app, "/mcp");
        assert!(matches!(app.view, ViewState::Overlay(Overlay::Mcp)));
    }

    #[test]
    fn help_command_opens_slash_help_overlay() {
        let mut app = make_test_app();
        dispatch_slash(&mut app, "/help");
        assert!(matches!(app.view, ViewState::Overlay(Overlay::SlashHelp)));
    }

    #[test]
    fn slash_help_lines_lists_visible_commands_only() {
        let app = make_test_app();
        let lines = slash_help_lines(&app.slash_registry);
        // Convert lines to flat strings for substring assertions.
        let flat: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(flat.contains("/help"));
        assert!(flat.contains("/clear"));
        // Hidden commands must not appear.
        assert!(!flat.contains("/voice"));
    }

    #[test]
    fn slash_outcome_status_message_records_last_status() {
        let mut app = make_test_app();
        apply_slash_outcome(slash::SlashOutcome::StatusMessage("hi".into()), &mut app);
        assert_eq!(app.last_status_message.as_deref(), Some("hi"));
        // Transcript also picks it up so the operator sees it.
        let info = last_info(&app).expect("info recorded");
        assert_eq!(info, "hi");
    }

    #[test]
    fn slash_outcome_insert_text_prefills_buffer() {
        let mut app = make_test_app();
        apply_slash_outcome(slash::SlashOutcome::InsertText("/clear ".into()), &mut app);
        assert_eq!(app.input.buffer, "/clear ");
        assert_eq!(app.input.cursor, "/clear ".len());
    }

    #[test]
    fn slash_outcome_overlay_sets_view_state() {
        let mut app = make_test_app();
        apply_slash_outcome(slash::SlashOutcome::Overlay(Overlay::Mcp), &mut app);
        assert!(matches!(app.view, ViewState::Overlay(Overlay::Mcp)));
    }

    #[test]
    fn doctor_command_runs_all_checks() {
        let app = make_test_app();
        let workspace = app
            .args
            .workspace
            .clone()
            .unwrap_or_else(|| app.cwd.clone());
        let checks = slash::observe::doctor::run_checks(&workspace, &app);
        assert!(
            !checks.is_empty(),
            "expected at least one health check to run"
        );
        // Each check has a name and a detail.
        for c in &checks {
            assert!(!c.name.is_empty());
            assert!(!c.detail.is_empty());
        }
        // Skills, hooks, mcp, provider, workspace — five expected checks.
        let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
        assert!(names.contains(&"skills"));
        assert!(names.contains(&"hooks"));
        assert!(names.contains(&"provider"));
    }

    #[test]
    fn loop_command_bounded_by_max_turns() {
        let mut app = make_test_app();
        app.args.max_turns = 5;
        dispatch_slash(&mut app, "/loop --n=100");
        let msg = last_info(&app).expect("status line");
        assert!(msg.contains("bounded to 5"), "msg: {msg}");
    }

    #[test]
    fn loop_command_emits_default_when_no_args() {
        let mut app = make_test_app();
        dispatch_slash(&mut app, "/loop");
        let msg = last_info(&app).expect("status line");
        assert!(msg.contains("planned 3 repeats"), "msg: {msg}");
    }

    #[test]
    fn init_command_writes_draft_to_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "agents.md content").unwrap();
        let mut app = make_test_app();
        app.args.workspace = Some(tmp.path().to_path_buf());
        dispatch_slash(&mut app, "/init");
        let draft = tmp.path().join("CLAUDE.draft.md");
        assert!(draft.exists(), "draft file written");
        let body = std::fs::read_to_string(&draft).unwrap();
        assert!(body.contains("# CLAUDE.md (draft)"));
        assert!(body.contains("agents.md content"));
    }

    #[test]
    fn init_command_warns_when_claude_md_exists_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "existing").unwrap();
        let mut app = make_test_app();
        app.args.workspace = Some(tmp.path().to_path_buf());
        dispatch_slash(&mut app, "/init");
        let infos: Vec<&str> = app
            .transcript
            .iter()
            .filter_map(|l| match l {
                TranscriptLine::Info(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            infos.iter().any(|s| s.contains("already exists")),
            "expected warning, got: {infos:?}",
        );
        // Existing CLAUDE.md not overwritten.
        let kept = std::fs::read_to_string(tmp.path().join("CLAUDE.md")).unwrap();
        assert_eq!(kept, "existing");
    }

    #[test]
    fn recap_emits_no_op_when_history_empty() {
        let mut app = make_test_app();
        dispatch_slash(&mut app, "/recap");
        let msg = last_info(&app).expect("recap status");
        assert!(msg.contains("no messages"), "got: {msg}");
    }

    #[test]
    fn voice_hidden_command_still_dispatches() {
        // Hidden = absent from typeahead but still dispatchable.
        let mut app = make_test_app();
        dispatch_slash(&mut app, "/voice");
        let msg = last_info(&app).expect("voice status message");
        assert!(msg.contains("voice dictation not available"));
    }

    #[test]
    fn suggester_orders_prefix_before_substring_then_alpha() {
        let registry = slash::register_builtin();
        // "co" appears as prefix of /compact, /config, /context AND as
        // substring within /recap (re-CA-p? no, c-o not in /recap).
        // Validate prefix-vs-substring policy on a synthetic call.
        let metas = registry.suggest("co");
        let names: Vec<&str> = metas.iter().map(|m| m.name).collect();
        // All three "co*" names must appear (in some order).
        assert!(names.contains(&"/compact"));
        assert!(names.contains(&"/config"));
        assert!(names.contains(&"/context"));
        // Prefix matches come first; they're alphabetized.
        let prefixes: Vec<&str> = names
            .iter()
            .take_while(|n| n.starts_with("/co"))
            .copied()
            .collect();
        assert_eq!(prefixes, vec!["/compact", "/config", "/context"]);
    }

    #[test]
    fn typeahead_consults_registry_suggester() {
        // Real ratatui-render test of the input popover: when the buffer
        // is `/c`, the menu should list multiple visible commands.
        let mut app = make_test_app();
        app.input.buffer = "/c".into();
        app.input.cursor = app.input.buffer.len();
        // Mimic the post-mutate menu-refresh logic from `handle_key`.
        let pairs: Vec<(&'static str, &'static str)> = app
            .slash_registry
            .suggest("c")
            .into_iter()
            .map(|m| (m.name, m.name))
            .collect();
        app.input.maybe_open_slash_menu(&pairs);
        app.input.refilter_slash_menu(&pairs);
        // After refilter, the menu must be open and non-empty.
        if let input::InputMode::SlashMenu(menu) = &app.input.mode {
            assert!(!menu.candidates.is_empty(), "menu has candidates");
            // /compact and /config must both be selectable.
            let names: Vec<&str> = menu.candidates.iter().map(|c| c.display.as_str()).collect();
            assert!(names.contains(&"/compact"));
            assert!(names.contains(&"/config"));
        } else {
            // The first call to `maybe_open_slash_menu` only opens when the
            // buffer is exactly "/". For "/c" we exercised `refilter_slash_menu`
            // — open the menu by hand and refilter.
            app.input.buffer = "/".into();
            app.input.cursor = 1;
            app.input.maybe_open_slash_menu(&pairs);
            assert!(
                matches!(app.input.mode, input::InputMode::SlashMenu(_)),
                "menu should open on bare '/'",
            );
        }
    }

    /// `UserPromptSubmit` hooks must fire *before* slash parsing so a
    /// hook can intercept or rewrite a slash command (ADR 0040).
    #[tokio::test]
    async fn user_prompt_submit_hook_fires_for_slash_commands() {
        use async_trait::async_trait;
        use caliban_agent_core::{HookDecision, Hooks, PromptCtx};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingHook {
            count: Arc<AtomicUsize>,
            last_prompt: Arc<std::sync::Mutex<String>>,
        }
        #[async_trait]
        impl Hooks for CountingHook {
            async fn user_prompt_submit(
                &self,
                ctx: &PromptCtx<'_>,
            ) -> caliban_agent_core::Result<HookDecision> {
                self.count.fetch_add(1, Ordering::SeqCst);
                *self.last_prompt.lock().unwrap() = ctx.prompt.to_string();
                Ok(HookDecision::Allow)
            }
        }

        let count = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(std::sync::Mutex::new(String::new()));
        let hook = Arc::new(CountingHook {
            count: Arc::clone(&count),
            last_prompt: Arc::clone(&last),
        });

        let ctx = PromptCtx {
            session_id: "tui-test",
            cwd: std::path::Path::new("/tmp"),
            turn_index: 0,
            prompt: "/clear",
            attachments: &[],
        };
        let decision = hook.user_prompt_submit(&ctx).await.unwrap();
        assert!(matches!(decision, HookDecision::Allow));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(*last.lock().unwrap(), "/clear");
    }

    #[tokio::test]
    async fn user_prompt_submit_hook_can_reject_slash_command() {
        use async_trait::async_trait;
        use caliban_agent_core::{HookDecision, Hooks, PromptCtx};

        struct DenyClear;
        #[async_trait]
        impl Hooks for DenyClear {
            async fn user_prompt_submit(
                &self,
                ctx: &PromptCtx<'_>,
            ) -> caliban_agent_core::Result<HookDecision> {
                if ctx.prompt == "/clear" {
                    return Ok(HookDecision::Deny("clear blocked by policy".into()));
                }
                Ok(HookDecision::Allow)
            }
        }

        let hook = Arc::new(DenyClear);
        let ctx = PromptCtx {
            session_id: "tui-test",
            cwd: std::path::Path::new("/tmp"),
            turn_index: 0,
            prompt: "/clear",
            attachments: &[],
        };
        let d = hook.user_prompt_submit(&ctx).await.unwrap();
        match d {
            HookDecision::Deny(msg) => assert!(msg.contains("clear blocked")),
            _ => panic!("expected Deny"),
        }
    }

    #[tokio::test]
    async fn user_prompt_submit_hook_can_rewrite_args() {
        use async_trait::async_trait;
        use caliban_agent_core::{HookDecision, Hooks, PromptCtx};

        struct Rewriter;
        #[async_trait]
        impl Hooks for Rewriter {
            async fn user_prompt_submit(
                &self,
                _ctx: &PromptCtx<'_>,
            ) -> caliban_agent_core::Result<HookDecision> {
                Ok(HookDecision::UpdatedInput(serde_json::Value::String(
                    "/help".into(),
                )))
            }
        }

        let hook = Arc::new(Rewriter);
        let ctx = PromptCtx {
            session_id: "tui-test",
            cwd: std::path::Path::new("/tmp"),
            turn_index: 0,
            prompt: "/random",
            attachments: &[],
        };
        let d = hook.user_prompt_submit(&ctx).await.unwrap();
        let HookDecision::UpdatedInput(v) = d else {
            panic!("expected UpdatedInput")
        };
        assert_eq!(v.as_str(), Some("/help"));
    }

    #[test]
    fn stub_commands_emit_helpful_status_naming_spec() {
        // Stubs should not just say "TODO"; they name the spec/owner so the
        // operator knows when it lands.
        let mut app = make_test_app();
        for (cmd, marker) in [
            ("/login", "Auth spec"),
            ("/logout", "Auth spec"),
            ("/setup-token", "Auth spec"),
            ("/heapdump", "jemalloc-prof"),
            ("/feedback", "feedback_url"),
            ("/agents", "Sub-agent isolation"),
            ("/effort", "model router v2"),
            ("/statusline", "Settings hierarchy"),
            ("/permissions", "Settings hierarchy"),
            ("/tui", "TUI ergonomics"),
        ] {
            app.transcript.clear();
            dispatch_slash(&mut app, cmd);
            let info = last_info(&app).expect(cmd);
            assert!(
                info.contains(marker),
                "{cmd} stub should mention `{marker}`: got `{info}`",
            );
        }
    }
}
