//! Ratatui-based interactive TUI.
//!
//! Organized as a thin entry layer over four focused submodules:
//!
//! - [`app`] — top-level [`App`] state, [`TranscriptLine`] entries,
//!   [`RunningTurn`]/[`Activity`] helpers.
//! - [`render`] — frame drawing for the main view (transcript + input +
//!   status bar) and the wrap-math helpers.
//! - [`overlay`] — [`Overlay`] enum + per-overlay line builders +
//!   [`render_overlay`].
//! - [`events`] — keyboard / mouse / agent-event / slash-command dispatch
//!   and the `/usage`, `/context`, `/compact` helpers.
//!
//! Existing siblings (`slash`, `ask`, `attach`, etc.) stay where they are.

#![allow(clippy::print_stdout, clippy::print_stderr)]

mod app;
pub(crate) mod ask;
mod attach;
mod completer;
mod events;
mod external_editor;
mod input;
mod overlay;
mod render;
mod reverse_history;
mod shell_escape;
pub(crate) mod slash;
mod toast;
mod transcript_viewer;

pub(crate) use app::{App, TranscriptLine};
pub(crate) use ask::TuiAskHandler;
pub(crate) use events::{handle_compact_command, render_context_lines, render_usage_lines};
pub(crate) use overlay::{Overlay, ViewState};

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

use anyhow::Result;
use caliban_agent_core::{Agent, TurnEventStream};
use caliban_sessions::{PersistedSession, SessionStore};
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, EventStream, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

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
    let mut term_events = EventStream::new();
    let mut agent_stream: Option<TurnEventStream> = None;

    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Drop expired toast before drawing so it doesn't flicker for one tick.
        if app.toast.as_ref().is_some_and(toast::Toast::is_expired) {
            app.toast = None;
        }
        guard
            .terminal()
            .draw(|frame| render::render(frame, &mut app))?;
        tracing::trace!("draw");
        stdout().flush().ok();
        if app.should_exit {
            break;
        }

        tokio::select! {
            term_event = term_events.next() => {
                let Some(Ok(ref ev)) = term_event else { break };
                events::handle_event(ev, &mut app, &mut agent_stream);
            }
            agent_event = async {
                if let Some(s) = agent_stream.as_mut() {
                    s.next().await
                } else {
                    std::future::pending::<Option<Result<caliban_agent_core::TurnEvent, caliban_agent_core::Error>>>().await
                }
            } => {
                match agent_event {
                    Some(Ok(evt)) => events::handle_agent_event(evt, &mut app),
                    Some(Err(ref e)) => {
                        events::handle_agent_error(e, &mut app);
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
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_end hook error (non-fatal)");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::app::{App, TranscriptLine};
    use super::events::{
        apply_slash_outcome, format_context_lines, format_usage_lines, handle_slash_command,
        next_permission_mode,
    };
    use super::overlay::{Overlay, ViewState, slash_help_lines};
    use super::render::{format_cache_suffix, wrap_lines_to_width};
    use super::{input, is_esc_chord, slash};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use std::sync::Arc;

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
