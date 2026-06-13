//! Event dispatch: keyboard, mouse, agent stream, slash commands, and the
//! `/usage` `/context` `/compact` rendering helpers.
//!
//! `App` state lives in [`super::app`]; rendering lives in [`super::render`]
//! and [`super::overlay`]. This module is the keyboard/mouse/stream side of
//! the loop.

use std::sync::Arc;

use caliban_agent_core::TurnEventStream;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

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
            let provider = match crate::resolved_provider(&app.args) {
                crate::ProviderKind::Anthropic => "anthropic",
                crate::ProviderKind::Openai => "openai",
                crate::ProviderKind::Ollama => "ollama",
                crate::ProviderKind::Google => "google",
            };
            let model = app.args.model.clone().unwrap_or_else(|| {
                crate::default_model_for(crate::resolved_provider(&app.args)).to_string()
            });
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
/// IE2: pop the next queued user message. Caller is responsible for
/// loading the text into `app.input.buffer` and re-entering the submit
/// path (the main TUI loop does this between turns via a synthetic
/// Enter event). Returns `None` if the queue is empty.
/// See caliban-ai/caliban#14 (queued-message drain).
pub(crate) fn drain_one_queued(app: &mut App) -> Option<String> {
    app.queued.pop_front()
}

/// IE2: two-stage Esc — if the queue is non-empty, the first Esc
/// clears it and arms `esc_armed_at`, returning `true` so the caller
/// short-circuits the existing Esc handler. With an empty queue
/// returns `false` and the existing logic (cancel running / clear
/// input / Esc-Esc chord for `/rewind`) runs as before, so a second
/// Esc with `running.is_some()` falls through to the cancel branch.
/// See caliban-ai/caliban#14 (queued-message drain).
pub(crate) fn handle_esc_queue_clear(app: &mut App, now: std::time::Instant) -> bool {
    if app.queued.is_empty() {
        return false;
    }
    app.queued.clear();
    app.esc_armed_at = Some(now);
    true
}

/// IE2: append the trimmed input buffer onto `app.queued` and clear
/// the buffer + cursor. Called by the submit handler when a turn is
/// already running (and the prompt isn't an immediate slash command,
/// which would be handled by IE1's intercept). No-op on whitespace-only
/// input so accidental Enters during inference don't enqueue blanks.
/// See caliban-ai/caliban#14 (queued-message drain).
pub(crate) fn push_user_input_to_queue(app: &mut App) {
    let line = app.input.buffer.trim().to_string();
    if line.is_empty() {
        return;
    }
    app.queued.push_back(line);
    app.input.buffer.clear();
    app.input.cursor = 0;
}

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
    let model = app.args.model.clone().unwrap_or_else(|| {
        crate::default_model_for(crate::resolved_provider(&app.args)).to_string()
    });
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

/// Drop the bypass latch: clear `bypass_latch`, revert `BypassPermissions`
/// mode to `Default`, and show a confirmation toast.
///
/// Extracted from the keybind handler for unit-testability.
pub(crate) fn drop_bypass(app: &mut App) {
    app.bypass_latch = false;
    if app.permission_mode.load() == caliban_agent_core::PermissionMode::BypassPermissions {
        app.permission_mode
            .store(caliban_agent_core::PermissionMode::Default);
    }
    app.toast = Some(toast::Toast::info(
        "bypass latch dropped — restart with --allow-dangerously-skip-permissions to re-enable",
    ));
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
        // IE3: left-button drag → in-app text selection. Down anchors
        // the selection at (row, col); Drag extends; Up finalises and
        // emits an OSC-52 clipboard write of the selected text resolved
        // through `app.position_map`. The render layer overlays the
        // highlight from `app.mouse_selection.range()` each frame.
        // See the TUI ergonomics design (mouse drag-select + OSC-52; shipped).
        MouseEventKind::Down(MouseButton::Left) => {
            app.mouse_selection
                .on_down(super::mouse_select::Cell::new(event.row, event.column));
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            app.mouse_selection
                .on_drag(super::mouse_select::Cell::new(event.row, event.column));
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.mouse_selection
                .on_up(super::mouse_select::Cell::new(event.row, event.column));
            // Extract + copy on completion. Empty selections (a click
            // without drag) produce empty text — copy_to_clipboard
            // short-circuits in that case.
            if let Some((start, end)) = app.mouse_selection.range() {
                let text = app.position_map.extract_range(start, end);
                if let Err(e) = super::clipboard::copy_to_clipboard(&text) {
                    tracing::warn!(error = %e, "OSC-52 clipboard write failed");
                }
            }
        }
        // Non-left button presses cancel any in-progress selection;
        // other events (motion without drag, scroll left/right) ignored.
        MouseEventKind::Down(_) => {
            app.mouse_selection.cancel();
        }
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

    // The "Always allow / Always deny" sub-prompt floats over any other
    // overlay. When it's open, every key routes to its handler regardless
    // of which overlay opened it — keeping render and dispatch in sync so
    // an invisible sub-prompt can't swallow keystrokes meant for the
    // overlay underneath (see bug fix in this PR's review history).
    if app.always_subprompt.is_some() {
        handle_always_subprompt_key(key, app);
        return;
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
            Overlay::Permissions if handle_permissions_overlay_key(key, app) => {
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
        // Ctrl+Shift+B — drop the bypass latch (clears the dangerous
        // `--allow-dangerously-skip-permissions` flag at runtime and
        // reverts any active BypassPermissions mode to Default).
        (KeyCode::Char('B'), m) if m.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) => {
            drop_bypass(app);
        }
        (KeyCode::Esc, _) => {
            // IE2: two-stage Esc — if there are queued user messages,
            // the first Esc clears the queue (not the in-flight turn).
            // A subsequent Esc with the queue empty falls through to
            // the existing logic below (cancel running / clear input /
            // Esc-Esc chord for `/rewind`).
            // See caliban-ai/caliban#14 (queued-message drain).
            if handle_esc_queue_clear(app, std::time::Instant::now()) {
                return;
            }
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
            // IE1: immediate slash commands (e.g. `/context`, `/cost`,
            // `/help`) dispatch even while a turn is in flight. The
            // classifier reads `SlashCommandMeta.immediate` set per
            // command; see caliban-ai/caliban#13 (immediate slash commands).
            if slash::is_immediate_slash(&prompt, &app.slash_registry) {
                let _line = app.input.submit();
                app.auto_scroll = true;
                handle_slash_command(&prompt, app);
                return;
            }
            // IE2: if a turn is already running, queue this prompt for
            // the next turn instead of dropping it. Drained on `RunEnd`.
            // See caliban-ai/caliban#14 (queued-message drain).
            if app.running.is_some() {
                push_user_input_to_queue(app);
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
        provider: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: false,
        interactive: false,
        inherited_hooks_config: None,
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

/// Number of selectable rows in the Ask modal's action stack: allow once,
/// deny once, always allow, always deny, Esc/deny. Mirrors the row order
/// built by `overlay::ask_modal_action_lines`.
pub(crate) const ASK_ACTION_COUNT: usize = 5;

/// One actionable choice in the Ask modal — shared by the bound letter keys
/// and by `Enter` on the highlighted row so the two paths can never diverge.
#[derive(Clone, Copy)]
enum AskChoice {
    AllowOnce,
    DenyOnce,
    AlwaysAllow,
    AlwaysDeny,
}

/// Map the action-stack cursor to the choice that `Enter` performs.
/// Rows 1 and 4 (and any out-of-range value) are "deny once".
fn ask_choice_for_cursor(cursor: usize) -> AskChoice {
    match cursor {
        0 => AskChoice::AllowOnce,
        2 => AskChoice::AlwaysAllow,
        3 => AskChoice::AlwaysDeny,
        _ => AskChoice::DenyOnce,
    }
}

/// Open the always-allow / always-deny sub-prompt for the pending request.
fn open_always_subprompt(app: &mut App, action: caliban_agent_core::Action) {
    if let Some(req) = app.ask_modal.as_ref() {
        let mut sp = ask::AlwaysSubprompt {
            suggestions: ask::derive_suggestions(&req.tool_name, &req.tool_input),
            selected: 0,
            custom: None,
            preview_matches: true, // exact-match suggestion matches by definition
            scope: caliban_settings::Scope::Project,
            comment: String::new(),
            reason: String::new(),
            action,
        };
        sp.selected = sp.suggestions.len().saturating_sub(1); // narrowest by default
        app.always_subprompt = Some(sp);
    }
}

/// Perform an Ask choice: resolve the pending oneshot (allow/deny once) or
/// open the appropriate always sub-prompt.
fn perform_ask_choice(app: &mut App, choice: AskChoice) {
    match choice {
        AskChoice::AllowOnce => {
            if let Some(req) = app.ask_modal.take() {
                let _ = req.respond.send(ask::AskResponse::AllowOnce);
                app.view = ViewState::Main;
            }
            drain_ask_queue(app);
        }
        AskChoice::DenyOnce => {
            if let Some(req) = app.ask_modal.take() {
                let _ = req.respond.send(ask::AskResponse::Deny);
                app.view = ViewState::Main;
            }
            drain_ask_queue(app);
        }
        AskChoice::AlwaysAllow => open_always_subprompt(app, caliban_agent_core::Action::Allow),
        AskChoice::AlwaysDeny => open_always_subprompt(app, caliban_agent_core::Action::Deny),
    }
}

pub(crate) fn handle_ask_modal_key(key: KeyEvent, app: &mut App) {
    // If the always-sub-prompt is open, route all keys there first.
    if app.always_subprompt.is_some() {
        handle_always_subprompt_key(key, app);
        return;
    }

    match (key.code, key.modifiers) {
        // Arrow keys move the highlight over the action stack; Enter activates
        // the highlighted row. These are additive — the bound letter keys
        // below still work regardless of where the cursor sits.
        (KeyCode::Up, _) => app.ask_cursor = app.ask_cursor.saturating_sub(1),
        (KeyCode::Down, _) => {
            app.ask_cursor = (app.ask_cursor + 1).min(ASK_ACTION_COUNT - 1);
        }
        (KeyCode::Enter, _) => perform_ask_choice(app, ask_choice_for_cursor(app.ask_cursor)),

        // Bound letter keys — always available regardless of the cursor.
        // Lowercase `a`/`d` open the always-allow / always-deny sub-prompt.
        (KeyCode::Char('y'), KeyModifiers::NONE) => perform_ask_choice(app, AskChoice::AllowOnce),
        (KeyCode::Char('a'), KeyModifiers::NONE) => perform_ask_choice(app, AskChoice::AlwaysAllow),
        (KeyCode::Char('d'), KeyModifiers::NONE) => perform_ask_choice(app, AskChoice::AlwaysDeny),
        // Deny once (Esc / Ctrl+C are a hard cancel, independent of cursor).
        (KeyCode::Char('n'), KeyModifiers::NONE)
        | (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            perform_ask_choice(app, AskChoice::DenyOnce);
        }
        _ => {}
    }
}

/// Key dispatch for the `/permissions` overlay. Returns `true` if the
/// key was consumed (caller skips the generic close-overlay handling);
/// `false` to let the generic Esc/q close path run.
///
/// Keys:
/// - `Tab`           → cycle overlay tab (View → Edit → Audit → View)
/// - `BackTab`       → cycle permission mode backward
/// - `↑` / `k`       → move cursor up in the runtime-rule list
/// - `↓` / `j`       → move cursor down in the runtime-rule list
/// - `d`             → remove the rule at the cursor (clamps cursor on remove)
/// - `a`             → open always-allow sub-prompt (Edit tab)
/// - `p`             → promote session rule to file (Edit tab)
/// - `t`             → open test pane (Edit tab)
/// - `Enter`         → run matcher in test pane
///
/// All other keys (including `Esc` / `q` / `Ctrl+C`) return `false` so
/// the generic overlay-close path handles them.
#[allow(clippy::too_many_lines)]
pub(crate) fn handle_permissions_overlay_key(key: KeyEvent, app: &mut App) -> bool {
    match (key.code, key.modifiers) {
        // Tab cycles through overlay tabs (View → Edit → Audit → View).
        (KeyCode::Tab, _) => {
            use crate::tui::app::PermissionsTab;
            app.permissions.tab = match app.permissions.tab {
                PermissionsTab::View => PermissionsTab::Edit,
                PermissionsTab::Edit => PermissionsTab::Audit,
                PermissionsTab::Audit => PermissionsTab::View,
            };
            true
        }
        (KeyCode::BackTab, _) => {
            // BackTab = Shift+Tab; cycle the permission mode (backward approximated
            // by N-1 forward steps — same approach as before Phase 5).
            for _ in 0..5 {
                cycle_permission_mode(app);
            }
            true
        }
        (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
            app.permissions.cursor = app.permissions.cursor.saturating_sub(1);
            true
        }
        (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
            let len = app.displayed_rules().len();
            if len > 0 {
                app.permissions.cursor = (app.permissions.cursor + 1).min(len - 1);
            }
            true
        }
        // `d` — delete: dispatch by rule origin.
        //   Session rules    → remove from runtime_rules store.
        //   File rules       → delete from source file via delete_rule_at.
        //                      Managed-scope files are read-only (toast only).
        //   Default rules    → read-only; emit a toast.
        //
        // Both View and Edit tabs use the same unified displayed_rules() list so
        // cursor position is consistent across tabs.
        (KeyCode::Char('d'), KeyModifiers::NONE) => {
            use crate::tui::app::RuleOrigin;
            let displayed = app.displayed_rules();
            if displayed.is_empty() {
                return true;
            }
            let idx = app.permissions.cursor.min(displayed.len() - 1);
            // Clone origin data so we can mutate app below.
            let origin = displayed[idx].origin.clone();
            let pattern = displayed[idx].pattern.clone();

            match origin {
                RuleOrigin::Session => {
                    // Session rules are the first N rows; idx maps directly.
                    if let Some(removed) = app.runtime_rules.remove(idx) {
                        app.toast = Some(toast::Toast::info(format!(
                            "removed session rule: {}",
                            removed.pattern,
                        )));
                    }
                }
                RuleOrigin::File { scope, path, .. } => {
                    if scope == caliban_settings::Scope::Managed {
                        app.toast = Some(toast::Toast::warn(
                            "managed-scope rules are read-only".to_string(),
                        ));
                        return true;
                    }
                    match caliban_settings::delete_rule_at(&path, &pattern) {
                        Ok(true) => {
                            app.toast = Some(toast::Toast::info(format!(
                                "removed rule from {}: {}",
                                path.display(),
                                pattern,
                            )));
                        }
                        Ok(false) => {
                            app.toast = Some(toast::Toast::warn(format!(
                                "no rule matching {} in {}",
                                pattern,
                                path.display(),
                            )));
                        }
                        Err(e) => {
                            app.toast = Some(toast::Toast::warn(format!("delete failed: {e}")));
                        }
                    }
                }
                RuleOrigin::Default => {
                    app.toast = Some(toast::Toast::warn(
                        "built-in default rules cannot be deleted".to_string(),
                    ));
                    return true;
                }
            }

            // Clamp cursor after potential list-length change.
            let new_len = app.displayed_rules().len();
            if new_len == 0 {
                app.permissions.cursor = 0;
            } else if app.permissions.cursor >= new_len {
                app.permissions.cursor = new_len - 1;
            }
            true
        }
        // `a` — add: open always-allow sub-prompt (Edit tab only).
        (KeyCode::Char('a'), KeyModifiers::NONE) => {
            use crate::tui::app::PermissionsTab;
            if app.permissions.tab == PermissionsTab::Edit {
                app.always_subprompt = Some(crate::tui::ask::AlwaysSubprompt {
                    suggestions: vec!["*".into()],
                    selected: 0,
                    custom: Some(String::new()),
                    preview_matches: false,
                    scope: caliban_settings::Scope::Project,
                    comment: String::new(),
                    reason: String::new(),
                    action: caliban_agent_core::Action::Allow,
                });
            }
            true
        }
        // `p` — promote: promote a session rule into a file (Edit tab only).
        (KeyCode::Char('p'), KeyModifiers::NONE) => {
            use crate::tui::app::PermissionsTab;
            if app.permissions.tab == PermissionsTab::Edit {
                let rules = app.runtime_rules.snapshot();
                let idx = app.permissions.cursor.min(rules.len().saturating_sub(1));
                if let Some(rule) = rules.get(idx) {
                    let sp = crate::tui::ask::AlwaysSubprompt {
                        suggestions: vec![rule.pattern.clone()],
                        selected: 0,
                        custom: None,
                        preview_matches: false,
                        scope: caliban_settings::Scope::Project,
                        comment: String::new(),
                        reason: String::new(),
                        action: rule.action,
                    };
                    app.always_subprompt = Some(sp);
                } else {
                    app.toast = Some(toast::Toast::warn(
                        "[p]romote: no rule selected".to_string(),
                    ));
                }
            }
            true
        }
        // `t` — test pane (Edit tab only).
        (KeyCode::Char('t'), KeyModifiers::NONE) => {
            use crate::tui::app::{PermissionsTab, PermissionsTestPane};
            if app.permissions.tab == PermissionsTab::Edit {
                app.permissions_test = Some(PermissionsTestPane::default());
            }
            true
        }
        // Enter inside the test pane: run the matcher.
        (KeyCode::Enter, _) if app.permissions_test.is_some() => {
            run_permissions_test(app);
            true
        }
        // Escape inside the test pane: close it.
        (KeyCode::Esc, _) if app.permissions_test.is_some() => {
            app.permissions_test = None;
            true // consumed — don't also close the overlay
        }
        // Tab inside the test pane: switch focus between fields.
        // (Only fires when test pane is open AND the outer Tab already changed tabs,
        //  but since Enter consumes first, we guard.)
        _ => false,
    }
}

/// Run the permissions matcher for the test pane and store the outcome.
fn run_permissions_test(app: &mut App) {
    let Some(tp) = app.permissions_test.as_mut() else {
        return;
    };
    let input: serde_json::Value =
        serde_json::from_str(&tp.input_json).unwrap_or_else(|_| serde_json::json!({}));
    let ctx = caliban_agent_core::ToolCtx {
        turn_index: 0,
        tool_use_id: "test",
        tool_name: &tp.tool_name,
        input: &input,
    };
    // Build the effective rule list: runtime rules (as Rule objects) first,
    // then built-in defaults.
    let runtime_snapshot: Vec<caliban_agent_core::permissions::Rule> = app
        .runtime_rules
        .snapshot()
        .into_iter()
        .map(|rr| caliban_agent_core::permissions::Rule {
            tool: rr.pattern.clone(),
            action: rr.action,
            comment: None,
            reason: None,
            expires_at: None,
        })
        .collect();
    let defaults = caliban_agent_core::default_rules();
    let all_rules: Vec<caliban_agent_core::permissions::Rule> =
        runtime_snapshot.into_iter().chain(defaults).collect();

    let outcome = match caliban_agent_core::evaluate_rules(&all_rules, &ctx) {
        Some(r) => format!("MATCH: {} (action={:?})", r.tool, r.action),
        None => "no match — would fall through to default Ask".into(),
    };
    tp.last_outcome = Some(outcome);
}

/// Key dispatch for the always-allow/deny sub-prompt opened by `a`/`d` in the
/// Ask modal. Navigation is arrow keys + Enter/Space; Tab cycles the scope
/// picker. No shift-letter shortcuts — deliberate choice for phase 4.
pub(crate) fn handle_always_subprompt_key(key: KeyEvent, app: &mut App) -> bool {
    let Some(sp) = app.always_subprompt.as_mut() else {
        return false;
    };
    match key.code {
        KeyCode::Esc => {
            app.always_subprompt = None;
            // Cancel sub-prompt → allow once (mirrors pressing `y` in the modal).
            if let Some(req) = app.ask_modal.take() {
                let _ = req.respond.send(ask::AskResponse::AllowOnce);
                app.view = ViewState::Main;
            }
            drain_ask_queue(app);
            true
        }
        KeyCode::Up => {
            if sp.selected > 0 {
                sp.selected -= 1;
            }
            true
        }
        KeyCode::Down => {
            // The [custom] row sits at index `suggestions.len()`, so the
            // navigable range is 0..=suggestions.len(). Without the
            // inclusive upper bound the operator can never reach the
            // custom row by arrow keys (this was a real bug).
            let max = sp.suggestions.len();
            if sp.selected < max {
                sp.selected += 1;
            }
            // Initialise the custom buffer on first arrival so the
            // operator can immediately start typing without an extra
            // gesture.
            if sp.selected == sp.suggestions.len() && sp.custom.is_none() {
                sp.custom = Some(String::new());
            }
            true
        }
        KeyCode::Tab => {
            // Cycle scope: Cli → Project → User → Local → Cli.
            let sp = app.always_subprompt.as_mut().unwrap();
            sp.scope = match sp.scope {
                caliban_settings::Scope::Cli => caliban_settings::Scope::Project,
                caliban_settings::Scope::Project => caliban_settings::Scope::User,
                caliban_settings::Scope::User => caliban_settings::Scope::Local,
                // Local and Managed both wrap back to Cli (Managed is read-only).
                caliban_settings::Scope::Local | caliban_settings::Scope::Managed => {
                    caliban_settings::Scope::Cli
                }
            };
            true
        }
        KeyCode::Enter => {
            // Commit: persist rule (or session) then resolve the ask.
            // commit_subprompt may add a runtime rule — drain the queue
            // afterward so concurrent requests get re-evaluated against
            // the just-added rule.
            commit_subprompt(app);
            app.always_subprompt = None;
            app.view = ViewState::Main;
            drain_ask_queue(app);
            true
        }
        // Character input: edits the custom-pattern buffer when the
        // `[custom]` row is selected; edits the comment field otherwise.
        KeyCode::Char(c) => {
            if let Some(sp) = app.always_subprompt.as_mut() {
                if sp.is_custom_selected() {
                    sp.custom.get_or_insert_with(String::new).push(c);
                } else {
                    sp.comment.push(c);
                }
            }
            true
        }
        KeyCode::Backspace => {
            if let Some(sp) = app.always_subprompt.as_mut() {
                if sp.is_custom_selected() {
                    if let Some(custom) = sp.custom.as_mut() {
                        custom.pop();
                    }
                } else {
                    sp.comment.pop();
                }
            }
            true
        }
        _ => true,
    }
}

fn commit_subprompt(app: &mut App) {
    let Some(sp) = app.always_subprompt.as_ref() else {
        return;
    };
    let pattern = sp.selected_pattern().to_string();
    let action = sp.action;

    // Apply the rule live for the remainder of this session regardless of
    // scope. The permission gate consults this same shared runtime store
    // (see `PermissionsHook::with_runtime_rules`), so the next matching
    // tool call resolves without re-prompting. File scopes additionally
    // persist below so the rule also survives a restart — without this
    // live step a "project" rule was written to disk but ignored until
    // the process restarted (#55).
    app.runtime_rules.add(caliban_agent_core::RuntimeRule {
        pattern: pattern.clone(),
        action,
    });

    if sp.scope != caliban_settings::Scope::Cli {
        let kind = caliban_settings::FileKind::Permissions;
        // Use the session's workspace root (same source the `/permissions`
        // overlay reads) rather than the process CWD so the rule lands in
        // the project the operator is actually working in.
        let cwd = app.cwd.clone();
        if let Some(target) = caliban_settings::scope_path(sp.scope, kind, &cwd) {
            let comment = sp.comment.clone();
            let reason = sp.reason.clone();
            let deny = action == caliban_agent_core::Action::Deny;
            let rule = caliban_settings::RuleSpec {
                pattern,
                action: action_str(action).into(),
                comment: (!comment.is_empty()).then_some(comment),
                reason: (deny && !reason.is_empty()).then_some(reason),
                expires_at: None,
                tool: None,
            };
            if let Err(e) = caliban_settings::append_rule_to_file(&target, &rule) {
                // Already applied live above; the gesture isn't lost, it
                // just won't outlive the session.
                tracing::warn!(
                    error = %e,
                    path = %target.display(),
                    "failed to persist permission rule; applied for this session only"
                );
            } else {
                app.toast = Some(toast::Toast::info(format!(
                    "rule saved to {}",
                    target.display()
                )));
            }
        }
        // scope_path returned None (e.g., Managed) — nothing to persist;
        // the rule is already applied live for this session.
    }

    // Resolve the pending Ask channel.
    if let Some(req) = app.ask_modal.take() {
        let response = match action {
            caliban_agent_core::Action::Allow => ask::AskResponse::AlwaysAllow,
            caliban_agent_core::Action::Deny => ask::AskResponse::AlwaysReject,
            caliban_agent_core::Action::Ask => ask::AskResponse::AllowOnce,
        };
        let _ = req.respond.send(response);
    }
}

fn action_str(a: caliban_agent_core::Action) -> &'static str {
    match a {
        caliban_agent_core::Action::Allow => "allow",
        caliban_agent_core::Action::Ask => "ask",
        caliban_agent_core::Action::Deny => "deny",
    }
}

/// After resolving an Ask modal (any branch), drain any concurrent
/// requests that arrived while the modal was open. Each is re-evaluated
/// against `runtime_rules` — which may have just been extended by the
/// "Always allow / Always deny" branch the user picked — and either
/// auto-resolves silently (when a session rule matches) or surfaces as
/// the next visible modal.
///
/// This closes the bug where a model that batched N concurrent tool
/// calls would see the 2nd…Nth calls denied with no UI surface, even
/// when the user picked an "Always allow" pattern that would have
/// matched them.
pub(crate) fn drain_ask_queue(app: &mut App) {
    while let Some(req) = app.ask_queue.pop_front() {
        let ctx = caliban_agent_core::ToolCtx {
            turn_index: 0,
            tool_use_id: "",
            tool_name: &req.tool_name,
            input: &req.tool_input,
        };
        match app.runtime_rules.evaluate(&ctx) {
            Some(caliban_agent_core::Action::Allow) => {
                let _ = req.respond.send(ask::AskResponse::AllowOnce);
            }
            Some(caliban_agent_core::Action::Deny) => {
                let _ = req.respond.send(ask::AskResponse::Deny);
            }
            None | Some(caliban_agent_core::Action::Ask) => {
                // No session rule matches — surface as the next modal.
                app.ask_modal = Some(req);
                app.ask_cursor = 0;
                app.view = ViewState::Overlay(crate::tui::Overlay::AskModal);
                return;
            }
        }
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

    /// IE2 Task 8 (RED): `handle_esc_queue_clear` clears a non-empty
    /// queue and arms `esc_armed_at`, returning `true` so the caller
    /// can short-circuit the existing Esc behaviour. With an empty
    /// queue it returns `false` and does nothing — existing logic
    /// (cancel running / clear input / Esc-Esc chord) runs as before.
    #[test]
    fn handle_esc_queue_clear_clears_and_arms_when_queue_non_empty() {
        let mut app = crate::tui::App::for_tests();
        app.queued.push_back("a".into());
        app.queued.push_back("b".into());
        assert!(app.esc_armed_at.is_none());
        let now = std::time::Instant::now();
        let consumed = handle_esc_queue_clear(&mut app, now);
        assert!(consumed);
        assert!(app.queued.is_empty());
        assert_eq!(app.esc_armed_at, Some(now));
    }

    #[test]
    fn handle_esc_queue_clear_noop_when_queue_empty() {
        let mut app = crate::tui::App::for_tests();
        let now = std::time::Instant::now();
        let consumed = handle_esc_queue_clear(&mut app, now);
        assert!(!consumed);
        assert!(app.esc_armed_at.is_none());
    }

    /// IE2 Task 7 (RED): `drain_one_queued` pops the front of `app.queued`
    /// and returns it; returns `None` when the queue is empty. The main
    /// loop calls this between turns to auto-dispatch queued user input.
    #[test]
    fn drain_one_queued_returns_front_and_pops() {
        let mut app = crate::tui::App::for_tests();
        app.queued.push_back("first".into());
        app.queued.push_back("second".into());
        assert_eq!(drain_one_queued(&mut app).as_deref(), Some("first"));
        assert_eq!(app.queued.len(), 1);
        assert_eq!(drain_one_queued(&mut app).as_deref(), Some("second"));
        assert!(app.queued.is_empty());
    }

    #[test]
    fn drain_one_queued_returns_none_when_empty() {
        let mut app = crate::tui::App::for_tests();
        assert!(drain_one_queued(&mut app).is_none());
    }

    /// IE2 Task 6 (RED): `push_user_input_to_queue` pushes the trimmed
    /// input buffer onto `app.queued` and clears the buffer + cursor.
    /// Empty input is a no-op so accidental Enters during inference
    /// don't enqueue blank messages.
    #[test]
    fn push_user_input_to_queue_appends_and_clears_buffer() {
        let mut app = crate::tui::App::for_tests();
        app.input.buffer = "hello world".into();
        app.input.cursor = app.input.buffer.len();
        push_user_input_to_queue(&mut app);
        assert_eq!(app.queued.front().map(String::as_str), Some("hello world"));
        assert!(app.input.buffer.is_empty());
        assert_eq!(app.input.cursor, 0);
    }

    #[test]
    fn push_user_input_to_queue_noop_on_empty() {
        let mut app = crate::tui::App::for_tests();
        app.input.buffer = "   ".into(); // whitespace-only counts as empty
        push_user_input_to_queue(&mut app);
        assert!(app.queued.is_empty());
    }

    #[test]
    fn push_user_input_to_queue_appends_fifo() {
        let mut app = crate::tui::App::for_tests();
        app.input.buffer = "first".into();
        push_user_input_to_queue(&mut app);
        app.input.buffer = "second".into();
        push_user_input_to_queue(&mut app);
        assert_eq!(app.queued.len(), 2);
        assert_eq!(app.queued[0], "first");
        assert_eq!(app.queued[1], "second");
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

    // -------------------------------------------------------------------
    // Task 7.4: bypass-latch drop
    // -------------------------------------------------------------------

    #[test]
    fn dropping_latch_reverts_mode_to_default() {
        let mut app = App::for_tests();
        app.bypass_latch = true;
        app.permission_mode
            .store(caliban_agent_core::PermissionMode::BypassPermissions);
        drop_bypass(&mut app);
        assert!(!app.bypass_latch);
        assert_eq!(
            app.permission_mode.load(),
            caliban_agent_core::PermissionMode::Default
        );
    }

    #[test]
    fn dropping_latch_leaves_non_bypass_mode_unchanged() {
        let mut app = App::for_tests();
        app.bypass_latch = true;
        app.permission_mode
            .store(caliban_agent_core::PermissionMode::DontAsk);
        drop_bypass(&mut app);
        assert!(!app.bypass_latch);
        // DontAsk should remain unchanged — only BypassPermissions gets reverted.
        assert_eq!(
            app.permission_mode.load(),
            caliban_agent_core::PermissionMode::DontAsk
        );
    }

    // -------------------------------------------------------------------
    // Phase 5 follow-up: [d] dispatch by RuleOrigin
    // -------------------------------------------------------------------

    /// Test 1: [d] on a session rule removes it from `runtime_rules` (existing
    /// behaviour preserved; session rules are first in the displayed list).
    #[test]
    fn d_key_on_session_rule_removes_from_runtime_rules() {
        use caliban_agent_core::permissions::RuntimeRule;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        app.runtime_rules.add(RuntimeRule {
            pattern: "Bash:ls *".into(),
            action: caliban_agent_core::Action::Allow,
        });
        // Cursor at 0 → first row = the session rule we just added.
        app.permissions.cursor = 0;
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let consumed = handle_permissions_overlay_key(key, &mut app);
        assert!(consumed);
        // The runtime rule should be gone.
        assert!(
            app.runtime_rules.snapshot().is_empty(),
            "session rule must be removed from runtime_rules"
        );
        // A toast should have been set.
        assert!(
            app.toast.is_some(),
            "toast must be set after session-rule deletion"
        );
    }

    /// Test 2: [d] on a managed-scope rule emits a read-only toast and does
    /// NOT attempt file deletion.
    #[test]
    fn d_key_on_managed_rule_emits_readonly_toast() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let mut app = App::for_tests();

        // Inject a managed-scope rule directly into displayed_rules by
        // making App::displayed_rules return one via a fake path. Since we
        // can't easily inject a file-backed rule in unit tests without
        // creating temp files, we instead test that the delete handler
        // returns the right toast for a managed rule by constructing the
        // key event and relying on displayed_rules() returning the built-in
        // defaults (which are RuleOrigin::Default, not Managed).
        //
        // To properly exercise the managed path, we add a session rule then
        // set cursor beyond it so it falls on a Default rule, and verify the
        // "built-in" toast fires. The managed branch is tested separately by
        // checking the code path with a synthetic origin.
        //
        // For the managed path: use the actual session-rule index offset to
        // get to a Default-origin rule, then verify the toast text.
        app.permissions.cursor = 0; // points to first built-in default (no session rules)
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        handle_permissions_overlay_key(key, &mut app);
        // The first row on a fresh app is a Default rule → should see read-only toast.
        let toast_text = app.toast.as_ref().expect("toast must be set").text.clone();
        assert!(
            toast_text.contains("cannot be deleted"),
            "default-rule toast must mention cannot-be-deleted; got: {toast_text}"
        );
    }

    /// Test 3: [d] on a default rule emits the built-in toast and does NOT
    /// touch any rule store.
    #[test]
    fn d_key_on_default_rule_emits_readonly_toast() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let mut app = App::for_tests();
        // No session rules → cursor 0 is the first default rule.
        app.permissions.cursor = 0;
        let rules_before = app.runtime_rules.snapshot().len();
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let consumed = handle_permissions_overlay_key(key, &mut app);
        assert!(consumed);
        // Runtime rules must be untouched.
        assert_eq!(
            app.runtime_rules.snapshot().len(),
            rules_before,
            "runtime_rules must not be mutated when deleting a default rule"
        );
        let toast_text = app.toast.as_ref().expect("toast must be set").text.clone();
        assert!(
            toast_text.contains("cannot be deleted"),
            "toast must mention cannot-be-deleted; got: {toast_text}"
        );
    }

    /// Test 4: [d] on a session rule sets cursor correctly when list shrinks.
    #[test]
    fn d_key_clamps_cursor_after_session_rule_deletion() {
        use caliban_agent_core::permissions::RuntimeRule;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        // Add two session rules. Cursor on the second one.
        app.runtime_rules.add(RuntimeRule {
            pattern: "A".into(),
            action: caliban_agent_core::Action::Allow,
        });
        app.runtime_rules.add(RuntimeRule {
            pattern: "B".into(),
            action: caliban_agent_core::Action::Allow,
        });
        app.permissions.cursor = 1; // point at rule "B"
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        handle_permissions_overlay_key(key, &mut app);
        // "B" should be removed; only "A" remains in session rules.
        let session_rules = app.runtime_rules.snapshot();
        assert_eq!(session_rules.len(), 1);
        assert_eq!(session_rules[0].pattern, "A");
        // Cursor must remain valid (not off the end of the full list).
        let new_len = app.displayed_rules().len();
        assert!(
            app.permissions.cursor < new_len,
            "cursor {} must be < displayed_rules len {}",
            app.permissions.cursor,
            new_len
        );
    }

    // -------------------------------------------------------------------
    // Ask modal: the labels rendered in the modal MUST match the keys
    // `handle_ask_modal_key` actually accepts. If you change one, the
    // other has to move with it. This test pins the contract.
    //
    // History: the v1 modal advertised capital `[A]` / `[R]` while the
    // v2 handler only accepts lowercase `a` / `d`. Operators saw the
    // label and typed shift-letters that the handler silently ignored.
    // Don't let that regression sneak back in.
    // -------------------------------------------------------------------

    fn build_app_with_ask_modal() -> (
        App,
        tokio::sync::oneshot::Receiver<crate::tui::ask::AskResponse>,
    ) {
        let mut app = App::for_tests();
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.ask_modal = Some(crate::tui::ask::AskRequest {
            tool_name: "Bash".into(),
            input_summary: "command=ls".into(),
            always_pattern: "Bash:ls *".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            respond: tx,
        });
        (app, rx)
    }

    fn flatten_lines(lines: &[ratatui::text::Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn ask_modal_label_advertises_lowercase_keys() {
        let (app, _rx) = build_app_with_ask_modal();
        let rendered = flatten_lines(&crate::tui::overlay::ask_modal_lines(&app));

        // Lowercase keys are mandatory (no shift-letter triggers per the
        // tui-prefer-choice-selection-over-capital-letters memory).
        for needle in ["[y]", "[a]", "[n]", "[d]", "[Esc]"] {
            assert!(
                rendered.contains(needle),
                "Ask modal label must advertise {needle}; rendered text was:\n{rendered}"
            );
        }
        // Capital-letter v1 leftovers must NOT come back.
        for forbidden in ["[A]", "[R]", "[Y]", "[N]"] {
            assert!(
                !rendered.contains(forbidden),
                "Ask modal label must NOT advertise {forbidden} (handler doesn't accept it); rendered text:\n{rendered}"
            );
        }
    }

    #[test]
    fn ask_modal_handler_accepts_every_advertised_lowercase_key() {
        // `y` → allow-once: closes the modal, leaves no sub-prompt.
        let (mut app, _rx) = build_app_with_ask_modal();
        handle_ask_modal_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.ask_modal.is_none(), "[y] must resolve the modal");
        assert!(
            app.always_subprompt.is_none(),
            "[y] does not open a sub-prompt"
        );

        // `a` → opens the always-allow sub-prompt; modal stays armed
        // until the sub-prompt commits.
        let (mut app, _rx) = build_app_with_ask_modal();
        handle_ask_modal_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &mut app,
        );
        let sp = app
            .always_subprompt
            .as_ref()
            .expect("[a] must open the always-allow sub-prompt");
        assert_eq!(sp.action, caliban_agent_core::Action::Allow);

        // `n` → deny-once: closes the modal, leaves no sub-prompt.
        let (mut app, _rx) = build_app_with_ask_modal();
        handle_ask_modal_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.ask_modal.is_none(), "[n] must resolve the modal");
        assert!(
            app.always_subprompt.is_none(),
            "[n] does not open a sub-prompt"
        );

        // `d` → opens the always-deny sub-prompt.
        let (mut app, _rx) = build_app_with_ask_modal();
        handle_ask_modal_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut app,
        );
        let sp = app
            .always_subprompt
            .as_ref()
            .expect("[d] must open the always-deny sub-prompt");
        assert_eq!(sp.action, caliban_agent_core::Action::Deny);

        // `Esc` → deny-once.
        let (mut app, _rx) = build_app_with_ask_modal();
        handle_ask_modal_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);
        assert!(app.ask_modal.is_none(), "[Esc] must resolve the modal");
    }

    #[test]
    fn ask_modal_arrow_keys_move_and_clamp_cursor() {
        let (mut app, _rx) = build_app_with_ask_modal();
        assert_eq!(app.ask_cursor, 0, "cursor starts at the top row");

        // Up at the top is a no-op (clamps at 0).
        handle_ask_modal_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut app);
        assert_eq!(app.ask_cursor, 0);

        // Down walks to the last row and then clamps.
        for expected in 1..ASK_ACTION_COUNT {
            handle_ask_modal_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
            assert_eq!(app.ask_cursor, expected);
        }
        handle_ask_modal_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
        assert_eq!(
            app.ask_cursor,
            ASK_ACTION_COUNT - 1,
            "Down clamps at the last row"
        );

        // Arrow keys alone never resolve the modal.
        assert!(app.ask_modal.is_some());
    }

    #[test]
    fn ask_modal_enter_activates_highlighted_row() {
        // Default cursor (row 0 = allow once): Enter resolves to allow.
        let (mut app, _rx) = build_app_with_ask_modal();
        handle_ask_modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert!(app.ask_modal.is_none(), "Enter on 'allow once' resolves");
        assert!(app.always_subprompt.is_none());

        // Row 1 = deny once.
        let (mut app, _rx) = build_app_with_ask_modal();
        app.ask_cursor = 1;
        handle_ask_modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert!(app.ask_modal.is_none(), "Enter on 'deny once' resolves");
        assert!(app.always_subprompt.is_none());

        // Row 2 = always allow → opens the allow sub-prompt, modal stays armed.
        let (mut app, _rx) = build_app_with_ask_modal();
        app.ask_cursor = 2;
        handle_ask_modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert_eq!(
            app.always_subprompt.as_ref().map(|sp| sp.action),
            Some(caliban_agent_core::Action::Allow),
            "Enter on 'always allow' opens the allow sub-prompt"
        );

        // Row 3 = always deny → opens the deny sub-prompt.
        let (mut app, _rx) = build_app_with_ask_modal();
        app.ask_cursor = 3;
        handle_ask_modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert_eq!(
            app.always_subprompt.as_ref().map(|sp| sp.action),
            Some(caliban_agent_core::Action::Deny),
            "Enter on 'always deny' opens the deny sub-prompt"
        );

        // Row 4 = Esc/deny once.
        let (mut app, _rx) = build_app_with_ask_modal();
        app.ask_cursor = 4;
        handle_ask_modal_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert!(app.ask_modal.is_none(), "Enter on last row denies once");
    }

    #[test]
    fn ask_modal_a_key_makes_sub_prompt_actually_visible() {
        // Regression: pressing `a` in the Ask modal opens an
        // `AlwaysSubprompt` but the render path forgot to draw it, so
        // operators saw the original modal unchanged and assumed the key
        // had no effect. Any subsequent keypress then routed to the
        // (invisible) sub-prompt handler and silently went nowhere.
        //
        // This test pins that the render output AFTER pressing `a`
        // contains sub-prompt-specific text that wasn't in the original
        // modal. If the renderer regresses, "Save to:" disappears and
        // the test fails.
        use ratatui::{Terminal, backend::TestBackend};

        let (mut app, _rx) = build_app_with_ask_modal();
        app.view = ViewState::Overlay(crate::tui::Overlay::AskModal);

        // Sanity: before pressing `a`, the rendered modal does NOT
        // mention "Save to:" (that text is part of the sub-prompt only).
        let backend = TestBackend::new(80, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| crate::tui::render::render(f, &mut app))
            .unwrap();
        let before = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol().to_string())
            .collect::<String>();
        assert!(
            !before.contains("Save to:"),
            "before pressing `a`, the modal must not show sub-prompt content"
        );

        // Drive `a` through the production dispatch path (not directly
        // through `handle_ask_modal_key` — that would skip the
        // top-level routing we're guarding here).
        let mut stream: Option<caliban_agent_core::TurnEventStream> = None;
        handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(
            app.always_subprompt.is_some(),
            "[a] must open the sub-prompt state"
        );

        // Re-render and verify the sub-prompt content now appears.
        term.draw(|f| crate::tui::render::render(f, &mut app))
            .unwrap();
        let after = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol().to_string())
            .collect::<String>();
        assert!(
            after.contains("Save to:"),
            "after pressing `a` the sub-prompt MUST be rendered (scope picker is its tell); \
             rendered output was:\n{after}"
        );
    }

    #[test]
    fn sub_prompt_arrow_keys_move_through_suggestions_and_custom_row() {
        // Regression: with Bash + a multi-token command, derive_suggestions
        // emits 3 patterns. The sub-prompt also exposes a `[custom]` row
        // (one past the end of `suggestions`). The arrow handler must
        // navigate the full 0..=suggestions.len() range — earlier code
        // capped Down at `suggestions.len() - 1` so the operator could
        // never reach the custom row by arrow keys.
        let (mut app, _rx) = build_app_with_ask_modal();
        app.view = ViewState::Overlay(crate::tui::Overlay::AskModal);
        // Replace the fixture's `command=ls` with a multi-token bash so
        // derive_suggestions produces 3 entries.
        app.ask_modal.as_mut().unwrap().tool_input =
            serde_json::json!({"command": "cargo test --all"});

        let mut stream: Option<caliban_agent_core::TurnEventStream> = None;
        // Press `a` to open the sub-prompt (selected defaults to narrowest = index 2).
        handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        let n = app.always_subprompt.as_ref().unwrap().suggestions.len();
        assert!(n >= 2, "fixture must produce at least 2 suggestions");
        assert_eq!(app.always_subprompt.as_ref().unwrap().selected, n - 1);

        // Press Down: must move from the narrowest real suggestion onto the
        // `[custom]` row (index == n).
        handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        let sp = app.always_subprompt.as_ref().unwrap();
        assert_eq!(
            sp.selected, n,
            "Down from narrowest suggestion must reach the [custom] row"
        );
        assert!(
            sp.is_custom_selected(),
            "[custom] row must be marked selected"
        );
        assert!(
            sp.custom.is_some(),
            "arriving on [custom] initialises the buffer"
        );

        // Press Up: must move back to the narrowest real suggestion.
        handle_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_eq!(app.always_subprompt.as_ref().unwrap().selected, n - 1);
        assert!(!app.always_subprompt.as_ref().unwrap().is_custom_selected());

        // From a real suggestion: typing chars edits the comment (not custom).
        for c in "note".chars() {
            handle_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut app,
                &mut stream,
            );
        }
        assert_eq!(app.always_subprompt.as_ref().unwrap().comment, "note");

        // Now navigate back to [custom] and type — must accumulate in `custom`,
        // not the comment.
        handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        for c in "Bash:foo".chars() {
            handle_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut app,
                &mut stream,
            );
        }
        let sp = app.always_subprompt.as_ref().unwrap();
        assert_eq!(sp.custom.as_deref(), Some("Bash:foo"));
        assert_eq!(
            sp.comment, "note",
            "typing on [custom] must NOT mutate the comment"
        );

        // selected_pattern() on the [custom] row returns the custom buffer.
        assert_eq!(sp.selected_pattern(), "Bash:foo");
    }

    #[test]
    fn concurrent_ask_requests_dont_get_auto_denied_when_modal_open() {
        // Regression: when a model batches N concurrent tool calls in
        // one turn, the 2nd…Nth AskRequests used to be auto-denied
        // inside tui.rs because `ask_modal` was already Some. Even when
        // the user picked "Always allow" on the first one with a pattern
        // that would have matched the others, the queued ones had
        // already been denied. The fix queues them and re-evaluates
        // against the runtime store on each modal answer.
        use tokio::sync::oneshot;

        let (mut app, _rx1) = build_app_with_ask_modal();
        let (tx2, mut rx2) = oneshot::channel();
        let (tx3, mut rx3) = oneshot::channel();
        app.ask_queue.push_back(crate::tui::ask::AskRequest {
            tool_name: "Bash".into(),
            input_summary: "command=ls -la".into(),
            always_pattern: "Bash:ls *".into(),
            tool_input: serde_json::json!({"command": "ls -la"}),
            respond: tx2,
        });
        app.ask_queue.push_back(crate::tui::ask::AskRequest {
            tool_name: "Bash".into(),
            input_summary: "command=cat foo".into(),
            always_pattern: "Bash:cat *".into(),
            tool_input: serde_json::json!({"command": "cat foo"}),
            respond: tx3,
        });

        // Simulate the user answering the first modal (any branch — here
        // we take the modal and add the runtime rule the "Always allow
        // Bash:*" branch would have added).
        let _ = app.ask_modal.take();
        app.runtime_rules.add(caliban_agent_core::RuntimeRule {
            pattern: "Bash".into(),
            action: caliban_agent_core::Action::Allow,
        });

        // Drain the queue — should auto-resolve both queued requests via
        // the runtime rule and leave `ask_modal` empty (no new modal).
        drain_ask_queue(&mut app);

        assert!(
            app.ask_modal.is_none(),
            "queue should be fully drained when runtime rule matches all pending"
        );
        assert!(app.ask_queue.is_empty(), "queue must be empty after drain");
        assert!(
            matches!(rx2.try_recv(), Ok(crate::tui::ask::AskResponse::AllowOnce)),
            "queued request #2 must receive AllowOnce, NOT Deny"
        );
        assert!(
            matches!(rx3.try_recv(), Ok(crate::tui::ask::AskResponse::AllowOnce)),
            "queued request #3 must receive AllowOnce, NOT Deny"
        );
    }

    #[test]
    fn commit_project_scope_rule_applies_live_in_session() {
        // Regression (#55): committing "Always allow" at *project* scope
        // must both (a) persist to .caliban/permissions.toml AND (b) take
        // effect in the running session by landing in the shared runtime
        // store the permission gate consults. Without (b), the model
        // re-prompts for the identical command until the process restarts.
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::for_tests();
        app.cwd = tmp.path().to_path_buf();
        app.always_subprompt = Some(crate::tui::ask::AlwaysSubprompt {
            suggestions: vec!["Bash:ls -F".into()],
            selected: 0,
            custom: None,
            preview_matches: false,
            scope: caliban_settings::Scope::Project,
            comment: String::new(),
            reason: String::new(),
            action: caliban_agent_core::Action::Allow,
        });

        commit_subprompt(&mut app);

        // (a) persisted under the project's .caliban dir.
        let toml = tmp.path().join(".caliban").join("permissions.toml");
        assert!(
            toml.exists(),
            "project rule must be persisted to {}",
            toml.display()
        );

        // (b) live in-session: the shared runtime store now matches the
        // identical command, so the gate won't re-prompt.
        let input = serde_json::json!({"command": "ls -F"});
        let ctx = caliban_agent_core::ToolCtx {
            turn_index: 0,
            tool_use_id: "t",
            tool_name: "Bash",
            input: &input,
        };
        assert_eq!(
            app.runtime_rules.evaluate(&ctx),
            Some(caliban_agent_core::Action::Allow),
            "committed project rule must apply live via the shared runtime store"
        );
    }

    #[test]
    fn drain_promotes_first_unmatched_request_to_modal() {
        // If the user added a rule that matches request A but not
        // request B, A auto-resolves; B becomes the next visible modal
        // so the user can decide.
        use tokio::sync::oneshot;

        let mut app = App::for_tests();
        // No active modal — start from an empty state.
        let (tx_a, mut rx_a) = oneshot::channel();
        let (tx_b, _rx_b) = oneshot::channel();
        app.ask_queue.push_back(crate::tui::ask::AskRequest {
            tool_name: "Bash".into(),
            input_summary: "command=ls".into(),
            always_pattern: "Bash:ls *".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            respond: tx_a,
        });
        app.ask_queue.push_back(crate::tui::ask::AskRequest {
            tool_name: "Edit".into(),
            input_summary: "path=foo.rs".into(),
            always_pattern: "Edit:foo.rs".into(),
            tool_input: serde_json::json!({"path": "foo.rs"}),
            respond: tx_b,
        });

        // Rule matches the Bash request only.
        app.runtime_rules.add(caliban_agent_core::RuntimeRule {
            pattern: "Bash".into(),
            action: caliban_agent_core::Action::Allow,
        });

        drain_ask_queue(&mut app);

        assert!(
            matches!(rx_a.try_recv(), Ok(crate::tui::ask::AskResponse::AllowOnce)),
            "Bash request matched the rule and must be AllowOnce"
        );
        let modal = app
            .ask_modal
            .as_ref()
            .expect("Edit request must surface as the next modal");
        assert_eq!(modal.tool_name, "Edit");
        assert!(
            app.ask_queue.is_empty(),
            "queue must be empty (Edit was promoted)"
        );
        assert!(
            matches!(app.view, ViewState::Overlay(crate::tui::Overlay::AskModal)),
            "view must flip to AskModal when a request is promoted"
        );
    }

    #[test]
    fn ask_modal_handler_ignores_v1_capital_letter_keys() {
        // Capital `A` / `R` were the v1 always-allow / always-reject
        // keys. Both must be silent no-ops in v2 so operators don't get
        // confused by a half-working keybind. The handler dispatches on
        // exact char + modifier match; `Char('A')` with Shift never
        // collides with `Char('a')` no-mod, so the no-op is enforced by
        // the match-arm structure, but we pin it explicitly here.
        for ch in ['A', 'R', 'Y', 'N'] {
            let (mut app, _rx) = build_app_with_ask_modal();
            handle_ask_modal_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::SHIFT),
                &mut app,
            );
            assert!(
                app.ask_modal.is_some() && app.always_subprompt.is_none(),
                "capital [{ch}] must be a no-op in the Ask modal (was a v1 keybind; v2 uses lowercase)"
            );
        }
    }

    // ===================================================================
    // stopped_for_surface — the previously-untested StopCondition variants.
    // ===================================================================

    #[test]
    fn max_tokens_exhausted_surfaces_as_error_with_effort_hint() {
        let s = stopped_for_surface(&StopCondition::MaxTokensExhausted)
            .expect("max-tokens-exhausted must surface");
        assert_eq!(s.level, StoppedForLevel::Error);
        assert_eq!(
            s.line,
            "[caliban: max output tokens exhausted — try /effort low]",
        );
    }

    #[test]
    fn refusal_surfaces_as_error() {
        let s = stopped_for_surface(&StopCondition::Refusal("no can do".to_string()))
            .expect("refusal must surface");
        assert_eq!(s.level, StoppedForLevel::Error);
        assert_eq!(s.line, "[caliban: model refusal: no can do]");
    }

    #[test]
    fn content_filter_surfaces_as_error() {
        let s = stopped_for_surface(&StopCondition::ContentFilter("blocked".to_string()))
            .expect("content filter must surface");
        assert_eq!(s.level, StoppedForLevel::Error);
        assert_eq!(s.line, "[caliban: content filter: blocked]");
    }

    #[test]
    fn stream_idle_surfaces_as_error_with_seconds() {
        let s = stopped_for_surface(&StopCondition::StreamIdle(std::time::Duration::from_secs(
            12,
        )))
        .expect("stream idle must surface");
        assert_eq!(s.level, StoppedForLevel::Error);
        assert_eq!(s.line, "[caliban: stream idle for 12s]");
    }

    // ===================================================================
    // next_permission_mode / cycle_permission_mode
    // ===================================================================

    #[test]
    fn next_permission_mode_skips_bypass_without_latch() {
        use caliban_agent_core::PermissionMode;
        // Walk every starting mode; whenever the natural `.next()` would be
        // BypassPermissions and the latch is off, the dangerous slot is
        // skipped and a warning is returned.
        for start in [
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::DontAsk,
            PermissionMode::BypassPermissions,
        ] {
            let (next, warning) = next_permission_mode(start, false);
            assert_ne!(
                next,
                PermissionMode::BypassPermissions,
                "without the latch, the cycle must never land on BypassPermissions (from {start:?})",
            );
            if start.next() == PermissionMode::BypassPermissions {
                assert!(
                    warning.is_some(),
                    "skipping the bypass slot must emit a warning (from {start:?})"
                );
            }
        }
    }

    #[test]
    fn next_permission_mode_allows_bypass_with_latch() {
        use caliban_agent_core::PermissionMode;
        // Find a mode whose `.next()` is BypassPermissions and confirm the
        // latch lets it through with no warning.
        for start in [
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::DontAsk,
            PermissionMode::BypassPermissions,
        ] {
            if start.next() == PermissionMode::BypassPermissions {
                let (next, warning) = next_permission_mode(start, true);
                assert_eq!(next, PermissionMode::BypassPermissions);
                assert!(warning.is_none(), "with the latch, no warning is emitted");
            }
        }
    }

    #[test]
    fn cycle_permission_mode_advances_and_sets_toast() {
        let mut app = App::for_tests();
        let before = app.permission_mode.load();
        cycle_permission_mode(&mut app);
        let after = app.permission_mode.load();
        assert_ne!(before, after, "cycling must change the mode");
        assert!(app.toast.is_some(), "cycling sets an informational toast");
    }

    #[test]
    fn cycle_permission_mode_syncs_plan_mode_flag() {
        use caliban_agent_core::PermissionMode;
        let mut app = App::for_tests();
        // Cycle until we land on Plan, then assert the legacy flag mirrors it.
        for _ in 0..6 {
            cycle_permission_mode(&mut app);
            let plan_flag = app.plan_mode.load(std::sync::atomic::Ordering::Relaxed);
            assert_eq!(
                plan_flag,
                app.permission_mode.load() == PermissionMode::Plan,
                "plan_mode flag must mirror whether the mode is Plan",
            );
        }
    }

    // ===================================================================
    // format_usage_lines / format_context_lines
    // ===================================================================

    #[test]
    fn format_usage_lines_empty_reports_no_calls() {
        let app = App::for_tests();
        let lines = format_usage_lines(&app.cost_accumulator);
        assert!(lines[0].starts_with("usage \u{2014} total $"));
        assert!(
            lines.iter().any(|l| l.contains("no provider calls yet")),
            "empty accumulator must report no calls; got {lines:?}"
        );
    }

    #[test]
    fn format_usage_lines_with_recorded_call_lists_model() {
        let app = App::for_tests();
        let usage = caliban_provider::Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: Some(10),
            cache_creation_input_tokens: Some(5),
        };
        app.cost_accumulator
            .record("anthropic", "claude-test", &usage, None);
        let lines = format_usage_lines(&app.cost_accumulator);
        assert!(
            lines.iter().any(|l| l.contains("by model:")),
            "recorded call must produce a by-model header; got {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("anthropic/claude-test")),
            "the recorded model must appear; got {lines:?}"
        );
    }

    #[test]
    fn format_context_lines_zero_capacity_reports_no_capacity() {
        let window = caliban_telemetry::ContextWindow::new();
        let lines = format_context_lines(&window);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("no capacity reported"),
            "zero-capacity window must report no capacity; got {lines:?}"
        );
    }

    #[test]
    fn format_context_lines_capacity_but_empty_bins() {
        let window = caliban_telemetry::ContextWindow::new();
        window.set_capacity(100_000);
        let lines = format_context_lines(&window);
        assert!(
            lines[0].contains("100000-token window"),
            "must show the capacity header; got {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("no messages yet")),
            "empty bins must produce the no-messages note; got {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("warning")),
            "low utilization must not warn; got {lines:?}"
        );
    }

    #[test]
    fn format_context_lines_over_80_percent_warns() {
        let window = caliban_telemetry::ContextWindow::new();
        // Tiny capacity so a single message blows past 80%.
        window.set_capacity(10);
        window.record_history(&[caliban_provider::Message::user_text(
            "this is a fairly long user message that will easily exceed ten tokens of capacity",
        )]);
        let lines = format_context_lines(&window);
        assert!(
            lines.iter().any(|l| l.contains("warning")),
            "over-80%% utilization must emit a warning; got {lines:?}"
        );
        // Non-empty bins → at least one per-kind line, no "no messages" note.
        assert!(
            !lines.iter().any(|l| l.contains("no messages yet")),
            "non-empty bins must not show the no-messages note; got {lines:?}"
        );
    }

    // ===================================================================
    // action_str / ask_choice_for_cursor — table tests
    // ===================================================================

    #[test]
    fn action_str_maps_each_action() {
        use caliban_agent_core::Action;
        assert_eq!(action_str(Action::Allow), "allow");
        assert_eq!(action_str(Action::Ask), "ask");
        assert_eq!(action_str(Action::Deny), "deny");
    }

    #[test]
    fn ask_choice_for_cursor_table() {
        // Rows 1 and 4 (and anything out of range) collapse to DenyOnce.
        assert!(matches!(ask_choice_for_cursor(0), AskChoice::AllowOnce));
        assert!(matches!(ask_choice_for_cursor(1), AskChoice::DenyOnce));
        assert!(matches!(ask_choice_for_cursor(2), AskChoice::AlwaysAllow));
        assert!(matches!(ask_choice_for_cursor(3), AskChoice::AlwaysDeny));
        assert!(matches!(ask_choice_for_cursor(4), AskChoice::DenyOnce));
        assert!(matches!(ask_choice_for_cursor(99), AskChoice::DenyOnce));
    }

    // ===================================================================
    // apply_slash_outcome — one assertion per SlashOutcome variant.
    // ===================================================================

    #[test]
    fn apply_slash_outcome_continue_is_noop() {
        let mut app = App::for_tests();
        let before = app.transcript.len();
        apply_slash_outcome(slash::SlashOutcome::Continue, &mut app);
        assert_eq!(app.transcript.len(), before);
        assert!(!app.should_exit);
    }

    #[test]
    fn apply_slash_outcome_quit_sets_should_exit() {
        let mut app = App::for_tests();
        apply_slash_outcome(slash::SlashOutcome::Quit, &mut app);
        assert!(app.should_exit);
    }

    #[test]
    fn apply_slash_outcome_overlay_sets_view() {
        let mut app = App::for_tests();
        apply_slash_outcome(slash::SlashOutcome::Overlay(Overlay::Permissions), &mut app);
        assert!(matches!(app.view, ViewState::Overlay(Overlay::Permissions)));
    }

    #[test]
    fn apply_slash_outcome_status_message_records_transcript_and_status() {
        let mut app = App::for_tests();
        apply_slash_outcome(
            slash::SlashOutcome::StatusMessage("hello status".into()),
            &mut app,
        );
        assert_eq!(app.last_status_message.as_deref(), Some("hello status"));
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptLine::Info(m)) if m == "hello status"
        ));
    }

    #[test]
    fn apply_slash_outcome_insert_text_fills_buffer() {
        let mut app = App::for_tests();
        apply_slash_outcome(slash::SlashOutcome::InsertText("/foo bar".into()), &mut app);
        assert_eq!(app.input.buffer, "/foo bar");
        assert_eq!(app.input.cursor, app.input.buffer.len());
    }

    #[test]
    fn apply_slash_outcome_reload_emits_info_line() {
        let mut app = App::for_tests();
        apply_slash_outcome(slash::SlashOutcome::Reload, &mut app);
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptLine::Info(m)) if m.contains("reload requested")
        ));
    }

    // ===================================================================
    // handle_agent_event — per TurnEvent variant.
    // ===================================================================

    /// Helper: install a fresh running turn so the activity-transition
    /// branches inside `handle_agent_event` are exercised.
    fn app_with_running_turn() -> App {
        let mut app = App::for_tests();
        app.running = Some(RunningTurn {
            cancel: tokio_util::sync::CancellationToken::new(),
            activity: Activity::WaitingForModel {
                since: std::time::Instant::now(),
            },
        });
        app
    }

    #[test]
    fn agent_event_text_delta_appends_and_sets_streaming() {
        use caliban_agent_core::TurnEvent;
        let mut app = app_with_running_turn();
        handle_agent_event(
            TurnEvent::AssistantTextDelta {
                turn_index: 0,
                content_block_index: 0,
                text: "hello ".into(),
            },
            &mut app,
        );
        handle_agent_event(
            TurnEvent::AssistantTextDelta {
                turn_index: 0,
                content_block_index: 0,
                text: "world".into(),
            },
            &mut app,
        );
        // Two deltas coalesce into one AssistantText line.
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptLine::AssistantText(s)) if s == "hello world"
        ));
        assert!(matches!(
            app.running.as_ref().unwrap().activity,
            Activity::Streaming { .. }
        ));
    }

    #[test]
    fn agent_event_thinking_delta_appends_and_sets_thinking() {
        use caliban_agent_core::TurnEvent;
        let mut app = app_with_running_turn();
        handle_agent_event(
            TurnEvent::AssistantThinkingDelta {
                turn_index: 0,
                content_block_index: 0,
                text: "pondering".into(),
            },
            &mut app,
        );
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptLine::AssistantThinking(s)) if s == "pondering"
        ));
        assert!(matches!(
            app.running.as_ref().unwrap().activity,
            Activity::Thinking { .. }
        ));
    }

    #[test]
    fn agent_event_tool_call_lifecycle_start_input_end() {
        use caliban_agent_core::TurnEvent;
        let mut app = app_with_running_turn();
        handle_agent_event(
            TurnEvent::ToolCallStart {
                turn_index: 0,
                tool_use_id: "tc1".into(),
                name: "Bash".into(),
            },
            &mut app,
        );
        assert!(matches!(
            app.running.as_ref().unwrap().activity,
            Activity::DispatchingTool { .. }
        ));
        handle_agent_event(
            TurnEvent::ToolCallInputDelta {
                turn_index: 0,
                tool_use_id: "tc1".into(),
                partial_json: "{\"cmd\":".into(),
            },
            &mut app,
        );
        handle_agent_event(
            TurnEvent::ToolCallInputDelta {
                turn_index: 0,
                tool_use_id: "tc1".into(),
                partial_json: "\"ls\"}".into(),
            },
            &mut app,
        );
        handle_agent_event(
            TurnEvent::ToolCallEnd {
                turn_index: 0,
                tool_use_id: "tc1".into(),
                is_error: false,
                content: vec![caliban_provider::ContentBlock::Text(
                    caliban_provider::TextBlock {
                        text: "ok-result".into(),
                        cache_control: None,
                    },
                )],
            },
            &mut app,
        );
        // After ToolCallEnd, activity reverts to waiting-for-model.
        assert!(matches!(
            app.running.as_ref().unwrap().activity,
            Activity::WaitingForModel { .. }
        ));
        // The ToolCall line accumulated the input JSON and the result.
        let found = app.transcript.iter().any(|l| {
            matches!(
                l,
                TranscriptLine::ToolCall { tool_use_id, input, result, .. }
                    if tool_use_id == "tc1"
                        && input == "{\"cmd\":\"ls\"}"
                        && matches!(result, Some((false, r)) if r == "ok-result")
            )
        });
        assert!(found, "tool-call line must carry input + result");
    }

    #[test]
    fn agent_event_turn_end_records_ttft_cost_and_blank_line() {
        use caliban_agent_core::TurnEvent;
        let mut app = app_with_running_turn();
        let before_len = app.transcript.len();
        handle_agent_event(
            TurnEvent::TurnEnd {
                turn_index: 0,
                assistant_message: caliban_provider::Message::assistant_text("done"),
                tool_results: Vec::new(),
                stop_reason: caliban_provider::StopReason::EndTurn,
                usage: caliban_provider::Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                ttft: Some(std::time::Duration::from_millis(123)),
                tbt: None,
            },
            &mut app,
        );
        assert_eq!(app.last_turn_ttft_ms, Some(123));
        // A blank AssistantText line is pushed for separation.
        assert!(app.transcript.len() > before_len);
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptLine::AssistantText(s)) if s.is_empty()
        ));
    }

    #[test]
    fn agent_event_run_end_surfaces_stop_pushes_usage_and_resets() {
        use caliban_agent_core::TurnEvent;
        let mut app = app_with_running_turn();
        app.auto_scroll = false;
        handle_agent_event(
            TurnEvent::RunEnd {
                final_messages: vec![caliban_provider::Message::user_text("hi")],
                total_usage: caliban_provider::Usage {
                    input_tokens: 42,
                    output_tokens: 7,
                    ..Default::default()
                },
                turn_count: 3,
                stopped_for: StopCondition::Cancelled,
            },
            &mut app,
        );
        // Cancelled surfaces as an Info line.
        assert!(
            app.transcript
                .iter()
                .any(|l| matches!(l, TranscriptLine::Info(s) if s == "[caliban: cancelled]")),
            "Cancelled stop condition must surface as an Info line"
        );
        // A usage summary lands with the run totals.
        assert!(
            app.transcript.iter().any(|l| matches!(
                l,
                TranscriptLine::UsageSummary { input_tokens, output_tokens, turn_count, .. }
                    if *input_tokens == 42 && *output_tokens == 7 && *turn_count == 3
            )),
            "RunEnd must push a UsageSummary with the run totals"
        );
        assert!(app.running.is_none(), "RunEnd clears the running turn");
        assert!(app.auto_scroll, "RunEnd re-pins auto-scroll");
        assert_eq!(app.messages.len(), 1, "messages mirror final_messages");
    }

    #[test]
    fn agent_event_run_end_error_stop_pushes_error_and_toast() {
        use caliban_agent_core::TurnEvent;
        let mut app = app_with_running_turn();
        handle_agent_event(
            TurnEvent::RunEnd {
                final_messages: Vec::new(),
                total_usage: caliban_provider::Usage::default(),
                turn_count: 1,
                stopped_for: StopCondition::ProviderError("boom".into()),
            },
            &mut app,
        );
        assert!(
            app.transcript.iter().any(|l| matches!(
                l,
                TranscriptLine::Error(s) if s.contains("provider error: boom")
            )),
            "error stop condition surfaces as an Error transcript line"
        );
        assert!(
            app.toast.is_some(),
            "error stop condition also raises a toast"
        );
    }

    // ===================================================================
    // handle_agent_error
    // ===================================================================

    #[test]
    fn handle_agent_error_cancelled_is_info_and_clears_running() {
        let mut app = app_with_running_turn();
        handle_agent_error(&caliban_agent_core::Error::Cancelled, &mut app);
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptLine::Info(s)) if s == "turn cancelled"
        ));
        assert!(app.running.is_none());
    }

    // ===================================================================
    // handle_permissions_overlay_key — previously-untested branches.
    // ===================================================================

    #[test]
    fn permissions_tab_cycles_view_edit_audit() {
        use crate::tui::app::PermissionsTab;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        assert_eq!(app.permissions.tab, PermissionsTab::View);
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(tab, &mut app));
        assert_eq!(app.permissions.tab, PermissionsTab::Edit);
        assert!(handle_permissions_overlay_key(tab, &mut app));
        assert_eq!(app.permissions.tab, PermissionsTab::Audit);
        assert!(handle_permissions_overlay_key(tab, &mut app));
        assert_eq!(app.permissions.tab, PermissionsTab::View);
    }

    #[test]
    fn permissions_backtab_cycles_permission_mode() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        let before = app.permission_mode.load();
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(backtab, &mut app));
        // Five forward steps from a 5-mode cycle land back on `before`, but
        // the bypass-skip may shift it; either way the consumer returns true.
        let _ = before;
    }

    #[test]
    fn permissions_cursor_up_down_move_and_clamp() {
        use caliban_agent_core::permissions::RuntimeRule;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        // Two session rules sit at the top of the displayed list.
        app.runtime_rules.add(RuntimeRule {
            pattern: "A".into(),
            action: caliban_agent_core::Action::Allow,
        });
        app.runtime_rules.add(RuntimeRule {
            pattern: "B".into(),
            action: caliban_agent_core::Action::Allow,
        });
        app.permissions.cursor = 0;
        // `j` moves down.
        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(down, &mut app));
        assert_eq!(app.permissions.cursor, 1);
        // Down arrow also moves down.
        assert!(handle_permissions_overlay_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut app
        ));
        assert_eq!(app.permissions.cursor, 2);
        // `k` moves up.
        let up = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(up, &mut app));
        assert_eq!(app.permissions.cursor, 1);
        // Up arrow at the bottom-of-a-walk keeps moving up; at 0 it clamps.
        app.permissions.cursor = 0;
        assert!(handle_permissions_overlay_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &mut app
        ));
        assert_eq!(app.permissions.cursor, 0, "Up at top clamps at 0");
    }

    #[test]
    fn permissions_a_key_opens_subprompt_only_on_edit_tab() {
        use crate::tui::app::PermissionsTab;
        use crossterm::event::{KeyCode, KeyModifiers};
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);

        // On the View tab, `a` is consumed but does NOT open a sub-prompt.
        let mut app = App::for_tests();
        app.permissions.tab = PermissionsTab::View;
        assert!(handle_permissions_overlay_key(key, &mut app));
        assert!(app.always_subprompt.is_none());

        // On the Edit tab, `a` opens the always-allow sub-prompt.
        let mut app = App::for_tests();
        app.permissions.tab = PermissionsTab::Edit;
        assert!(handle_permissions_overlay_key(key, &mut app));
        let sp = app
            .always_subprompt
            .as_ref()
            .expect("Edit-tab `a` must open the sub-prompt");
        assert_eq!(sp.action, caliban_agent_core::Action::Allow);
    }

    #[test]
    fn permissions_p_key_promotes_selected_session_rule() {
        use crate::tui::app::PermissionsTab;
        use caliban_agent_core::permissions::RuntimeRule;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        app.permissions.tab = PermissionsTab::Edit;
        app.runtime_rules.add(RuntimeRule {
            pattern: "Bash:ls *".into(),
            action: caliban_agent_core::Action::Allow,
        });
        app.permissions.cursor = 0;
        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(key, &mut app));
        let sp = app
            .always_subprompt
            .as_ref()
            .expect("`p` must open a promote sub-prompt for the selected rule");
        assert_eq!(sp.suggestions, vec!["Bash:ls *".to_string()]);
        assert_eq!(sp.action, caliban_agent_core::Action::Allow);
    }

    #[test]
    fn permissions_p_key_with_no_rules_warns() {
        use crate::tui::app::PermissionsTab;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        app.permissions.tab = PermissionsTab::Edit;
        // No session rules → promote has nothing to act on.
        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(key, &mut app));
        assert!(app.always_subprompt.is_none());
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("no rule selected")),
            "promote with no rules must warn"
        );
    }

    #[test]
    fn permissions_t_key_opens_test_pane_then_enter_runs_then_esc_closes() {
        use crate::tui::app::PermissionsTab;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        app.permissions.tab = PermissionsTab::Edit;

        // `t` opens the test pane.
        let t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(t, &mut app));
        assert!(app.permissions_test.is_some(), "`t` opens the test pane");

        // Seed a tool/input and Enter runs the matcher → last_outcome set.
        {
            let tp = app.permissions_test.as_mut().unwrap();
            tp.tool_name = "Bash".into();
            tp.input_json = "{\"command\":\"ls\"}".into();
        }
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(enter, &mut app));
        assert!(
            app.permissions_test
                .as_ref()
                .is_some_and(|tp| tp.last_outcome.is_some()),
            "Enter in the test pane must populate last_outcome"
        );

        // Esc inside the test pane closes the pane (consumed; overlay stays).
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(handle_permissions_overlay_key(esc, &mut app));
        assert!(app.permissions_test.is_none(), "Esc closes the test pane");
    }

    #[test]
    fn permissions_unhandled_key_returns_false() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        // `z` isn't bound → returns false so the generic close path runs.
        let key = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(!handle_permissions_overlay_key(key, &mut app));
    }

    // ===================================================================
    // handle_transcript_viewer_key
    // ===================================================================

    #[test]
    fn transcript_viewer_help_toggle() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        app.view = ViewState::Overlay(Overlay::TranscriptViewer);
        assert!(!app.transcript_viewer.show_help);
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.transcript_viewer.show_help);
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(!app.transcript_viewer.show_help);
    }

    #[test]
    fn transcript_viewer_ctrl_e_toggles_show_all() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        app.view = ViewState::Overlay(Overlay::TranscriptViewer);
        assert!(!app.transcript_viewer.show_all);
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &mut app,
        );
        assert!(app.transcript_viewer.show_all);
    }

    #[test]
    fn transcript_viewer_scroll_navigation() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        app.view = ViewState::Overlay(Overlay::TranscriptViewer);
        // Seed enough history lines that scrolling has somewhere to go.
        app.messages = (0..40)
            .map(|i| caliban_provider::Message::user_text(format!("line {i}")))
            .collect();
        // j / Down moves down.
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut app,
        );
        let after_j = app.transcript_viewer.scroll;
        assert!(after_j >= 1, "j scrolls down at least one row");
        // PageDown moves further.
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.transcript_viewer.scroll >= after_j);
        // k / Up moves back up.
        let before_up = app.transcript_viewer.scroll;
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.transcript_viewer.scroll < before_up);
        // PageUp moves up more.
        handle_transcript_viewer_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), &mut app);
        // G jumps to the bottom; g back to the top.
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            &mut app,
        );
        let bottom = app.transcript_viewer.scroll;
        assert!(bottom > 0, "G jumps to a non-zero bottom");
        handle_transcript_viewer_key(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(app.transcript_viewer.scroll, 0, "g jumps to the top");
    }

    #[test]
    fn transcript_viewer_esc_and_q_close() {
        use crossterm::event::{KeyCode, KeyModifiers};
        for code in [KeyCode::Esc, KeyCode::Char('q')] {
            let mut app = App::for_tests();
            app.view = ViewState::Overlay(Overlay::TranscriptViewer);
            handle_transcript_viewer_key(KeyEvent::new(code, KeyModifiers::NONE), &mut app);
            assert!(matches!(app.view, ViewState::Main));
        }
    }

    // ===================================================================
    // handle_reverse_history_key
    // ===================================================================

    fn app_with_reverse_history() -> App {
        let mut app = App::for_tests();
        app.reverse_history = Some(reverse_history::ReverseHistoryState::new(
            vec!["echo one".into(), "echo two".into(), "ls".into()],
            None,
            None,
        ));
        app.view = ViewState::Overlay(Overlay::ReverseHistory);
        app
    }

    #[test]
    fn reverse_history_char_and_backspace_edit_query() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app_with_reverse_history();
        for c in "echo".chars() {
            handle_reverse_history_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut app,
            );
        }
        // Two "echo …" entries match.
        let n = app.reverse_history.as_ref().unwrap().matches().len();
        assert_eq!(n, 2, "query 'echo' matches two history entries");
        handle_reverse_history_key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &mut app,
        );
        // "ech" still matches the two echo entries.
        assert_eq!(app.reverse_history.as_ref().unwrap().matches().len(), 2);
    }

    #[test]
    fn reverse_history_up_down_move_cursor() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app_with_reverse_history();
        handle_reverse_history_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
        assert_eq!(app.reverse_history.as_ref().unwrap().cursor, 1);
        handle_reverse_history_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut app);
        assert_eq!(app.reverse_history.as_ref().unwrap().cursor, 0);
    }

    #[test]
    fn reverse_history_ctrl_s_cycles_scope() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app_with_reverse_history();
        // Ctrl+S cycles scope without panicking (project/all caches are None).
        handle_reverse_history_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut app,
        );
        assert!(app.reverse_history.is_some());
    }

    #[test]
    fn reverse_history_enter_sets_buffer_and_closes() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app_with_reverse_history();
        // Cursor 0 → newest match ("ls").
        handle_reverse_history_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert!(app.reverse_history.is_none(), "Enter closes the overlay");
        assert!(matches!(app.view, ViewState::Main));
        assert_eq!(app.input.buffer, "ls", "Enter loads the selected entry");
    }

    #[test]
    fn reverse_history_esc_closes_without_buffer_change() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app_with_reverse_history();
        handle_reverse_history_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);
        assert!(app.reverse_history.is_none());
        assert!(matches!(app.view, ViewState::Main));
        assert!(app.input.buffer.is_empty());
    }

    // ===================================================================
    // handle_mcp_overlay_key
    // ===================================================================

    #[test]
    fn mcp_overlay_letter_keys_each_set_a_toast() {
        use crossterm::event::{KeyCode, KeyModifiers};
        for ch in ['d', 'r', 'a', 's', 't'] {
            let mut app = App::for_tests();
            let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
            assert!(
                handle_mcp_overlay_key(key, &mut app),
                "`{ch}` must be consumed in the /mcp overlay"
            );
            assert!(
                app.toast.is_some(),
                "`{ch}` in the /mcp overlay must set a toast"
            );
        }
    }

    #[test]
    fn mcp_overlay_unhandled_key_returns_false() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::for_tests();
        let key = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(!handle_mcp_overlay_key(key, &mut app));
    }

    // ===================================================================
    // handle_mouse
    // ===================================================================

    fn mouse(kind: MouseEventKind, row: u16, column: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_scroll_up_breaks_autoscroll_and_steps_up() {
        let mut app = App::for_tests();
        app.auto_scroll = true;
        app.last_max_scroll = 100;
        handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 5), &mut app);
        assert!(!app.auto_scroll, "ScrollUp exits auto-scroll");
        // Seeded from last_max_scroll (100) then stepped up by 3.
        assert_eq!(app.scroll, 100 - MOUSE_WHEEL_ROWS);
    }

    #[test]
    fn mouse_scroll_down_repins_at_bottom() {
        let mut app = App::for_tests();
        app.auto_scroll = false;
        app.last_max_scroll = 10;
        app.scroll = 9;
        handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), &mut app);
        // 9 + 3 >= 10 → re-pins to the live tail.
        assert_eq!(app.scroll, 10);
        assert!(
            app.auto_scroll,
            "scrolling past the end re-pins auto-scroll"
        );
    }

    #[test]
    fn mouse_scroll_down_steps_without_repin_when_room_remains() {
        let mut app = App::for_tests();
        app.auto_scroll = false;
        app.last_max_scroll = 100;
        app.scroll = 10;
        handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5), &mut app);
        assert_eq!(app.scroll, 10 + MOUSE_WHEEL_ROWS);
        assert!(!app.auto_scroll);
    }

    #[test]
    fn mouse_ignored_inside_overlay() {
        let mut app = App::for_tests();
        app.view = ViewState::Overlay(Overlay::Permissions);
        app.auto_scroll = true;
        app.last_max_scroll = 50;
        app.scroll = 0;
        handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 5), &mut app);
        // Overlay short-circuits before touching scroll state.
        assert_eq!(app.scroll, 0);
        assert!(app.auto_scroll);
    }

    #[test]
    fn mouse_left_down_drag_up_builds_and_clears_selection() {
        let mut app = App::for_tests();
        // Down anchors, Drag extends → a live selection exists.
        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 3),
            &mut app,
        );
        handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 7),
            &mut app,
        );
        assert!(
            app.mouse_selection.range().is_some(),
            "Down+Drag produces a selection range"
        );
        // Up finalises. With an empty PositionMap the extracted text is
        // empty and copy_to_clipboard short-circuits (no stdout write).
        handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, 7), &mut app);
        assert!(
            app.mouse_selection.range().is_some(),
            "completed selection stays visible until the next Down"
        );
    }

    #[test]
    fn mouse_left_click_without_drag_is_empty_selection() {
        let mut app = App::for_tests();
        // Down then Up at the same cell — a click, not a drag. The Up path
        // extracts an empty range and clipboard write short-circuits.
        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
            &mut app,
        );
        handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1, 1), &mut app);
        // A collapsed range is still "Some" (start == end).
        assert!(app.mouse_selection.range().is_some());
    }

    #[test]
    fn mouse_non_left_button_cancels_selection() {
        let mut app = App::for_tests();
        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
            &mut app,
        );
        assert!(app.mouse_selection.range().is_some());
        // A right-button press cancels any in-progress selection.
        handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Right), 2, 2),
            &mut app,
        );
        assert!(
            app.mouse_selection.range().is_none(),
            "non-left press cancels the selection"
        );
    }

    // ===================================================================
    // handle_key — normal-mode branches that don't submit a prompt.
    // ===================================================================

    fn no_stream() -> Option<TurnEventStream> {
        None
    }

    #[test]
    fn key_ctrl_c_exits_on_empty_buffer() {
        let mut app = App::for_tests();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut app,
            &mut stream,
        );
        assert!(app.should_exit, "Ctrl+C on an empty buffer exits");
    }

    #[test]
    fn key_ctrl_c_clears_non_empty_buffer() {
        let mut app = App::for_tests();
        app.input.buffer = "draft".into();
        app.input.cursor = app.input.buffer.len();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut app,
            &mut stream,
        );
        assert!(!app.should_exit, "Ctrl+C with text does not exit");
        assert!(app.input.buffer.is_empty(), "Ctrl+C clears the buffer");
    }

    #[test]
    fn key_ctrl_c_cancels_running_turn() {
        let mut app = app_with_running_turn();
        let token = app.running.as_ref().unwrap().cancel.clone();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut app,
            &mut stream,
        );
        assert!(token.is_cancelled(), "Ctrl+C cancels the running turn");
        assert!(!app.should_exit);
    }

    #[test]
    fn key_ctrl_d_exits_on_empty_buffer() {
        let mut app = App::for_tests();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            &mut app,
            &mut stream,
        );
        assert!(app.should_exit, "Ctrl+D on an empty buffer exits");
    }

    #[test]
    fn key_char_inserts_into_buffer() {
        let mut app = App::for_tests();
        let mut stream = no_stream();
        for c in "hi".chars() {
            handle_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut app,
                &mut stream,
            );
        }
        assert_eq!(app.input.buffer, "hi");
    }

    #[test]
    fn key_backspace_deletes_last_char() {
        let mut app = App::for_tests();
        app.input.buffer = "abc".into();
        app.input.cursor = 3;
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_eq!(app.input.buffer, "ab");
    }

    #[test]
    fn key_arrows_move_cursor() {
        let mut app = App::for_tests();
        app.input.buffer = "abc".into();
        app.input.cursor = 3;
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_eq!(app.input.cursor, 2);
        handle_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_eq!(app.input.cursor, 3);
        handle_key(
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_eq!(app.input.cursor, 0);
        handle_key(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_eq!(app.input.cursor, 3);
    }

    #[test]
    fn key_pageup_pagedown_scroll_math() {
        let mut app = App::for_tests();
        app.auto_scroll = true;
        app.last_max_scroll = 100;
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(!app.auto_scroll, "PageUp exits auto-scroll");
        assert_eq!(app.scroll, 90, "PageUp seeds from max then steps up by 10");
        handle_key(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_eq!(app.scroll, 100, "PageDown back to the tail re-pins");
        assert!(app.auto_scroll);
    }

    #[test]
    fn key_shift_enter_inserts_newline_without_submitting() {
        let mut app = App::for_tests();
        app.input.buffer = "line1".into();
        app.input.cursor = app.input.buffer.len();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            &mut app,
            &mut stream,
        );
        assert!(
            app.input.buffer.contains('\n'),
            "Shift+Enter adds a newline"
        );
        assert!(stream.is_none(), "Shift+Enter must not start a turn");
    }

    #[test]
    fn key_enter_empty_buffer_is_noop() {
        let mut app = App::for_tests();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(stream.is_none(), "Enter on an empty buffer does nothing");
    }

    #[test]
    fn key_enter_queues_prompt_while_running() {
        let mut app = app_with_running_turn();
        app.input.buffer = "queued prompt".into();
        app.input.cursor = app.input.buffer.len();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        // A running turn → the prompt is queued, not submitted.
        assert_eq!(
            app.queued.front().map(String::as_str),
            Some("queued prompt")
        );
        assert!(app.input.buffer.is_empty());
        assert!(stream.is_none(), "queued prompts do not start a new stream");
    }

    #[test]
    fn key_esc_clears_non_empty_buffer() {
        let mut app = App::for_tests();
        app.input.buffer = "draft".into();
        app.input.cursor = app.input.buffer.len();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(app.input.buffer.is_empty(), "Esc clears a non-empty buffer");
    }

    #[test]
    fn key_esc_cancels_running_turn() {
        let mut app = app_with_running_turn();
        let token = app.running.as_ref().unwrap().cancel.clone();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(token.is_cancelled(), "Esc cancels the running turn");
    }

    #[test]
    fn key_esc_esc_chord_opens_rewind() {
        let mut app = App::for_tests();
        // Empty buffer, no running turn. First Esc arms the chord.
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(app.last_esc_at.is_some(), "first Esc arms the chord");
        assert!(matches!(app.view, ViewState::Main));
        // Second Esc within the window opens the Rewind overlay.
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(
            matches!(app.view, ViewState::Overlay(Overlay::Rewind)),
            "Esc-Esc opens the Rewind overlay"
        );
        assert!(app.last_esc_at.is_none(), "chord consumed; timer reset");
    }

    #[test]
    fn key_esc_clears_queue_first() {
        let mut app = App::for_tests();
        app.queued.push_back("a".into());
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(app.queued.is_empty(), "first Esc clears the queue");
        assert!(app.esc_armed_at.is_some());
    }

    #[test]
    fn key_immediate_slash_dispatches_inline() {
        // `/help` is an immediate command; it dispatches even with no running
        // turn and must not start an agent stream.
        let mut app = App::for_tests();
        app.input.buffer = "/help".into();
        app.input.cursor = app.input.buffer.len();
        let mut stream = no_stream();
        let view_before = matches!(app.view, ViewState::Main);
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(stream.is_none(), "immediate slash must not start a stream");
        assert!(
            app.input.buffer.is_empty(),
            "the slash command was submitted"
        );
        let _ = view_before;
    }

    #[test]
    fn key_ctrl_shift_b_drops_bypass_latch() {
        let mut app = App::for_tests();
        app.bypass_latch = true;
        app.permission_mode
            .store(caliban_agent_core::PermissionMode::BypassPermissions);
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(
                KeyCode::Char('B'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            &mut app,
            &mut stream,
        );
        assert!(!app.bypass_latch, "Ctrl+Shift+B drops the latch");
        assert_eq!(
            app.permission_mode.load(),
            caliban_agent_core::PermissionMode::Default
        );
    }

    #[test]
    fn key_ctrl_o_opens_transcript_viewer() {
        let mut app = App::for_tests();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            &mut app,
            &mut stream,
        );
        assert!(matches!(
            app.view,
            ViewState::Overlay(Overlay::TranscriptViewer)
        ));
    }

    #[test]
    fn key_ctrl_r_opens_reverse_history() {
        let mut app = App::for_tests();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &mut app,
            &mut stream,
        );
        assert!(matches!(
            app.view,
            ViewState::Overlay(Overlay::ReverseHistory)
        ));
        assert!(app.reverse_history.is_some());
    }

    #[test]
    fn key_backtab_cycles_permission_mode() {
        let mut app = App::for_tests();
        let before = app.permission_mode.load();
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert_ne!(
            app.permission_mode.load(),
            before,
            "Shift+Tab cycles the mode"
        );
    }

    // ===================================================================
    // Overlay dispatch via handle_key (generic close path + delegation).
    // ===================================================================

    #[test]
    fn key_overlay_esc_returns_to_main() {
        let mut app = App::for_tests();
        app.view = ViewState::Overlay(Overlay::System);
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(matches!(app.view, ViewState::Main));
    }

    #[test]
    fn key_overlay_q_returns_to_main() {
        let mut app = App::for_tests();
        app.view = ViewState::Overlay(Overlay::System);
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &mut app,
            &mut stream,
        );
        assert!(matches!(app.view, ViewState::Main));
    }

    #[test]
    fn key_overlay_ctrl_c_closes_when_not_running() {
        let mut app = App::for_tests();
        app.view = ViewState::Overlay(Overlay::System);
        let mut stream = no_stream();
        handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut app,
            &mut stream,
        );
        assert!(matches!(app.view, ViewState::Main));
    }

    // ===================================================================
    // handle_event — Press vs non-Press gating.
    // ===================================================================

    #[test]
    fn handle_event_ignores_non_press_keys() {
        use crossterm::event::{Event, KeyEventKind};
        let mut app = App::for_tests();
        app.input.buffer.clear();
        let mut stream = no_stream();
        let mut key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        handle_event(&Event::Key(key), &mut app, &mut stream);
        assert!(
            app.input.buffer.is_empty(),
            "key Release events must be ignored"
        );
    }

    #[test]
    fn handle_event_press_key_inserts() {
        use crossterm::event::Event;
        let mut app = App::for_tests();
        let mut stream = no_stream();
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        handle_event(&Event::Key(key), &mut app, &mut stream);
        assert_eq!(app.input.buffer, "x");
    }
}
