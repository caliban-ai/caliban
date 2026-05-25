//! Top-level [`App`] state struct, [`TranscriptLine`] entries, and the
//! [`RunningTurn`] / [`Activity`] helpers that drive the status bar.
//!
//! This module owns the in-memory state of the TUI; rendering lives in
//! [`super::render`] and event dispatch lives in [`super::events`].

use std::path::PathBuf;
use std::sync::Arc;

use caliban_agent_core::Agent;
use caliban_sessions::{PersistedSession, SessionStore};

use super::ViewState;
use super::ask;
use super::input;
use super::reverse_history;
use super::slash;
use super::toast;
use super::transcript_viewer;
use crate::Args;

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
    pub(crate) fn label(&self) -> String {
        match self {
            Self::WaitingForModel { .. } => "waiting for model".into(),
            Self::Streaming { .. } => "streaming response".into(),
            Self::Thinking { .. } => "thinking".into(),
            Self::DispatchingTool { name, .. } => format!("running {name}"),
        }
    }

    pub(crate) fn since(&self) -> std::time::Instant {
        match self {
            Self::WaitingForModel { since }
            | Self::Streaming { since }
            | Self::Thinking { since }
            | Self::DispatchingTool { since, .. } => *since,
        }
    }
}

/// Pick a frame from a Braille spinner based on elapsed time. ~10 Hz advance.
pub(crate) fn spinner_frame(elapsed: std::time::Duration) -> char {
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
                    target: caliban_common::tracing_targets::TARGET_COST,
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
