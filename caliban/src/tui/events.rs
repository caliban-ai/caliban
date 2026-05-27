//! Event dispatch: keyboard, mouse, agent stream, slash commands, and the
//! `/usage` `/context` `/compact` rendering helpers.
//!
//! `App` state lives in [`super::app`]; rendering lives in [`super::render`]
//! and [`super::overlay`]. This module is the keyboard/mouse/stream side of
//! the loop.

use std::sync::Arc;

use caliban_agent_core::TurnEventStream;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};

use super::ViewState;
use super::app::{Activity, App, RunningTurn, TranscriptLine};
use super::ask;
use super::attach;
use super::external_editor;
use super::input::InputMode;
use super::is_esc_chord;
use super::overlay::Overlay;
use super::reverse_history;
use super::shell_escape;
use super::slash;
use super::toast;
use super::transcript_viewer;

// === Agent event handlers ===

/// Severity of a [`StoppedForSurface`] — controls whether the surface
/// renders as a red transcript [`TranscriptLine::Error`] + toast or a
/// neutral [`TranscriptLine::Info`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoppedForLevel {
    /// Provider error / hook denial / compaction failure.
    Error,
    /// Max-turns / cancelled.
    Info,
}

/// One-line description of a non-`EndOfTurn` [`caliban_agent_core::StopCondition`]
/// suitable for the transcript / toast surface. Pure (no `App` dependency)
/// so the mapping is unit-testable.
#[derive(Debug, Clone)]
pub(crate) struct StoppedForSurface {
    /// The user-visible message, wrapped in `[caliban: …]` framing per the
    /// 2026-05-25 LM Studio probe Findings 5 + 9.
    pub(crate) line: String,
    /// Whether to render this as a red error or a neutral info line.
    pub(crate) level: StoppedForLevel,
}

/// Map a `StopCondition` to a transcript / toast surface. Returns `None`
/// for `EndOfTurn` (the default natural stop) so callers can no-op.
pub(crate) fn stopped_for_surface(
    stopped_for: &caliban_agent_core::StopCondition,
) -> Option<StoppedForSurface> {
    use caliban_agent_core::StopCondition;
    match stopped_for {
        StopCondition::EndOfTurn => None,
        StopCondition::ProviderError(msg) => Some(StoppedForSurface {
            line: format!("[caliban: provider error: {msg}]"),
            level: StoppedForLevel::Error,
        }),
        StopCondition::HookDenied(msg) => Some(StoppedForSurface {
            line: format!("[caliban: hook denied: {msg}]"),
            level: StoppedForLevel::Error,
        }),
        StopCondition::CompactionFailed(msg) => Some(StoppedForSurface {
            line: format!("[caliban: compaction failed: {msg}]"),
            level: StoppedForLevel::Error,
        }),
        StopCondition::MaxTurnsReached(n) => Some(StoppedForSurface {
            line: format!("[caliban: max-turns ({n}) reached]"),
            level: StoppedForLevel::Info,
        }),
        StopCondition::Cancelled => Some(StoppedForSurface {
            line: "[caliban: cancelled]".to_string(),
            level: StoppedForLevel::Info,
        }),
        StopCondition::MaxTokensExhausted => Some(StoppedForSurface {
            line: "[caliban: max output tokens exhausted — try /effort low]".to_string(),
            level: StoppedForLevel::Error,
        }),
        StopCondition::Refusal(msg) => Some(StoppedForSurface {
            line: format!("[caliban: model refusal: {msg}]"),
            level: StoppedForLevel::Error,
        }),
        StopCondition::ContentFilter(msg) => Some(StoppedForSurface {
            line: format!("[caliban: content filter: {msg}]"),
            level: StoppedForLevel::Error,
        }),
        StopCondition::StreamIdle(d) => Some(StoppedForSurface {
            line: format!("[caliban: stream idle for {}s]", d.as_secs()),
            level: StoppedForLevel::Error,
        }),
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn handle_agent_event(evt: caliban_agent_core::TurnEvent, app: &mut App) {
    use caliban_agent_core::TurnEvent;
    // Per-event hot path: each delta / tool-call boundary fires here. The
    // `?evt` arg records a `&dyn Debug` over `TurnEvent` (which transitively
    // walks `Vec<ContentBlock>` / `Vec<Message>` etc.), so even though
    // `tracing` itself defers `Debug::fmt`, gate the macro entry behind the
    // callsite check to skip the valueset construction when DEBUG is off.
    if tracing::enabled!(tracing::Level::DEBUG) {
        tracing::debug!(?evt, "agent event");
    }
    match evt {
        TurnEvent::TurnStart { .. } => {
            // Keep the WaitingForModel activity; refreshed on first delta below.
        }
        TurnEvent::AssistantTextDelta { text, .. } => {
            app.last_delta_at = std::time::Instant::now();
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
            app.last_delta_at = std::time::Instant::now();
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
            app.last_delta_at = std::time::Instant::now();
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
            app.last_delta_at = std::time::Instant::now();
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
            stopped_for,
        } => {
            // Surface non-EndOfTurn stop conditions so the user sees *why*
            // the turn ended (Findings 5 + 9 from the 2026-05-25 LM Studio
            // probe). The pure helper returns `None` for the EndOfTurn
            // default; everything else becomes a transcript line + an
            // optional toast.
            if let Some(surface) = stopped_for_surface(&stopped_for) {
                match surface.level {
                    StoppedForLevel::Error => {
                        app.transcript
                            .push(TranscriptLine::Error(surface.line.clone()));
                        app.toast = Some(toast::Toast::error(surface.line));
                    }
                    StoppedForLevel::Info => {
                        app.transcript.push(TranscriptLine::Info(surface.line));
                    }
                }
            }
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

pub(crate) fn handle_agent_error(e: &caliban_agent_core::Error, app: &mut App) {
    tracing::warn!(error = %e, "agent error");
    if matches!(e, caliban_agent_core::Error::Cancelled) {
        app.transcript
            .push(TranscriptLine::Info("turn cancelled".into()));
    } else {
        app.transcript.push(TranscriptLine::Error(e.to_string()));
    }
    app.running = None;
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
pub(crate) fn handle_slash_command(line: &str, app: &mut App) {
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
pub(crate) fn format_usage_lines(cost: &caliban_telemetry::CostAccumulator) -> Vec<String> {
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
pub(crate) fn format_context_lines(window: &caliban_telemetry::ContextWindow) -> Vec<String> {
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

pub(crate) fn handle_event(
    event: &crossterm::event::Event,
    app: &mut App,
    agent_stream: &mut Option<TurnEventStream>,
) {
    use crossterm::event::Event;
    // Per-event hot path: each key / mouse move / resize fires here. The
    // `?event` arg records a `&dyn Debug` over `crossterm::event::Event`
    // (an enum with multi-field key/mouse variants); gate the macro entry
    // behind the callsite check so the disabled path is a single atomic
    // load with no valueset construction.
    if tracing::enabled!(tracing::Level::TRACE) {
        tracing::trace!(?event, "term event");
    }
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
pub(crate) const MOUSE_WHEEL_ROWS: u16 = 3;

pub(crate) fn handle_mouse(event: MouseEvent, app: &mut App) {
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
pub(crate) fn handle_key(key: KeyEvent, app: &mut App, agent_stream: &mut Option<TurnEventStream>) {
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
            // Phase C: per-server actions. We render the toasts and let
            // the manager-level wiring catch up in v2.1 (full disable /
            // reload / OAuth-from-key requires McpClientManager
            // mutability beyond the scope of this PR). For now the keys
            // surface as informative toasts so the operator knows the
            // contract.
            Overlay::Mcp if handle_mcp_overlay_key(key, app) => {
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
pub(crate) fn handle_ctrl_g(app: &mut App) {
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
pub(crate) fn open_reverse_history(app: &mut App) {
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
pub(crate) fn dispatch_shell_escape(command: &str, app: &mut App) {
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
/// Key dispatch for the `/mcp` overlay (ADR 0023 Phase C). Returns `true`
/// when the key was handled and the dispatcher should not fall through to
/// the generic Esc/q close branch.
///
/// The actual reload / disable / OAuth start operations require mutating
/// `McpClientManager`, which is owned outside the TUI loop. Until that
/// plumbing lands (v2.1), the keys surface as toasts so the contract is
/// discoverable while the underlying transitions are still external.
pub(crate) fn handle_mcp_overlay_key(key: KeyEvent, app: &mut App) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Char('d'), KeyModifiers::NONE) => {
            app.toast = Some(toast::Toast::warn(
                "mcp: disable not yet wired — edit `disabled = true` in mcp.toml then restart",
            ));
            true
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) => {
            app.toast = Some(toast::Toast::warn(
                "mcp: live reload not yet wired — restart caliban to pick up edits",
            ));
            true
        }
        (KeyCode::Char('a'), KeyModifiers::NONE) => {
            app.toast = Some(toast::Toast::info(
                "mcp: OAuth flow auto-triggers on first call when oauth=auto or manual",
            ));
            true
        }
        (KeyCode::Char('s'), KeyModifiers::NONE) => {
            app.toast = Some(toast::Toast::info(
                "mcp: stderr is logged to RUST_LOG=caliban::mcp::stderr",
            ));
            true
        }
        (KeyCode::Char('t'), KeyModifiers::NONE) => {
            app.toast = Some(toast::Toast::info(
                "mcp: tool list shown in the transcript on /usage and via tool dispatcher",
            ));
            true
        }
        _ => false,
    }
}

pub(crate) fn handle_ask_modal_key(key: KeyEvent, app: &mut App) {
    let response = match (key.code, key.modifiers) {
        (KeyCode::Char('y'), KeyModifiers::NONE) | (KeyCode::Enter, _) => {
            Some(ask::AskResponse::AllowOnce)
        }
        // Capital A / R bind to "Always allow / Always reject" — match
        // the spec's modal layout. The "Always" branches append a
        // session-scoped runtime rule via the agent's RuntimeRuleStore.
        (KeyCode::Char('A'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            Some(ask::AskResponse::AlwaysAllow)
        }
        (KeyCode::Char('R'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            Some(ask::AskResponse::AlwaysReject)
        }
        (KeyCode::Char('n'), KeyModifiers::NONE)
        | (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(ask::AskResponse::Deny),
        _ => None,
    };
    if let Some(r) = response
        && let Some(req) = app.ask_modal.take()
    {
        // Persist the derived pattern as a session-scoped runtime rule
        // when the user chose an "Always" branch. The store lives on
        // App (added in Task 11) so plain hooks composition keeps
        // working — no need to rewire PermissionsHook.
        match r {
            ask::AskResponse::AlwaysAllow => {
                app.runtime_rules.add(caliban_agent_core::RuntimeRule {
                    pattern: req.always_pattern.clone(),
                    action: caliban_agent_core::Action::Allow,
                });
            }
            ask::AskResponse::AlwaysReject => {
                app.runtime_rules.add(caliban_agent_core::RuntimeRule {
                    pattern: req.always_pattern.clone(),
                    action: caliban_agent_core::Action::Deny,
                });
            }
            _ => {}
        }
        let _ = req.respond.send(r);
        app.view = ViewState::Main;
    }
}

/// Key dispatch for the Ctrl+O transcript viewer overlay.
pub(crate) fn handle_transcript_viewer_key(key: KeyEvent, app: &mut App) {
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
pub(crate) fn handle_reverse_history_key(key: KeyEvent, app: &mut App) {
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
pub(crate) fn refresh_at_menu(app: &mut App) {
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
    use caliban_agent_core::StopCondition;

    // -------------------------------------------------------------------
    // RunEnd.stopped_for surfacing (Findings 5 + 9 from the 2026-05-25
    // LM Studio probe). The runloop populates `stopped_for` correctly;
    // these tests cover the TUI's *mapping* from that field to a
    // user-visible transcript / toast line. Driving a full `App` is
    // high-friction (many required handles), so we test the pure
    // helper that `handle_agent_event` delegates to.
    // -------------------------------------------------------------------

    #[test]
    fn end_of_turn_is_silent() {
        assert!(stopped_for_surface(&StopCondition::EndOfTurn).is_none());
    }

    #[test]
    fn provider_error_surfaces_as_error_with_caliban_framing() {
        let s = stopped_for_surface(&StopCondition::ProviderError(
            "HTTP 400: context length exceeded".to_string(),
        ))
        .expect("provider error must surface");
        assert_eq!(s.level, StoppedForLevel::Error);
        assert_eq!(
            s.line,
            "[caliban: provider error: HTTP 400: context length exceeded]",
        );
    }

    #[test]
    fn hook_denied_and_compaction_failed_surface_as_error() {
        let hd = stopped_for_surface(&StopCondition::HookDenied(
            "policy: 'Bash' blocked".to_string(),
        ))
        .expect("hook denial must surface");
        assert_eq!(hd.level, StoppedForLevel::Error);
        assert_eq!(hd.line, "[caliban: hook denied: policy: 'Bash' blocked]");

        let cf = stopped_for_surface(&StopCondition::CompactionFailed(
            "out of budget".to_string(),
        ))
        .expect("compaction failure must surface");
        assert_eq!(cf.level, StoppedForLevel::Error);
        assert_eq!(cf.line, "[caliban: compaction failed: out of budget]");
    }

    #[test]
    fn max_turns_and_cancelled_surface_as_info() {
        let mt = stopped_for_surface(&StopCondition::MaxTurnsReached(7))
            .expect("max-turns must surface");
        assert_eq!(mt.level, StoppedForLevel::Info);
        assert_eq!(mt.line, "[caliban: max-turns (7) reached]");

        let c = stopped_for_surface(&StopCondition::Cancelled).expect("cancellation must surface");
        assert_eq!(c.level, StoppedForLevel::Info);
        assert_eq!(c.line, "[caliban: cancelled]");
    }
}
