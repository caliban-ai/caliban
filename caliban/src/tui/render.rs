//! Frame rendering: main view layout, transcript wrap math, status bar,
//! and the `format_*` helpers that turn structured data into displayable text.
//!
//! Overlay drawing lives in [`super::overlay`]; this module only handles
//! the main view (transcript + input + status) and the helpers needed to
//! lay it out.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use super::ViewState;
use super::app::{App, TranscriptLine, spinner_frame};
use super::input::{self, InputMode};
use super::overlay::render_overlay;
use super::toast;

#[allow(clippy::too_many_lines)]
pub(crate) fn render(frame: &mut ratatui::Frame<'_>, app: &mut App) {
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

    // IE2: the toast strip is also borrowed for the QUEUED indicator
    // when no toast is active and `app.queued` is non-empty. Toast
    // wins when both are present (errors > queue hint).
    let toast_rows: u16 =
        u16::from(app.toast.as_ref().is_some_and(|t| !t.is_expired()) || !app.queued.is_empty());
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

    // IE3: build the per-frame position map from the rendered transcript
    // cells, then overlay the mouse-selection highlight (if any) on top
    // of the same area. Done in this order so the position map reflects
    // the *original* glyphs (used for clipboard extract), and the user
    // sees the highlight over the chars they're about to copy. See
    // `docs/TODO.md` § TUI ergonomics § IE3.
    let transcript_area = chunks[0];
    {
        let buf = frame.buffer_mut();
        record_transcript_cells_into_position_map(buf, transcript_area, &mut app.position_map);
        if let Some(range) = app.mouse_selection.range() {
            let highlight = Style::default().bg(Color::DarkGray).fg(Color::White);
            apply_selection_highlight(buf, transcript_area, range, highlight);
        }
    }

    // chunks[1] = top horizontal rule
    let hrule_top = Block::default()
        .borders(Borders::TOP)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hrule_top, chunks[1]);

    // chunks[2] = ephemeral toast or QUEUED indicator (zero rows when
    // neither is active). Toast wins when both are present.
    if toast_rows == 1 {
        if let Some(t) = &app.toast {
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
        } else if let Some(text) = format_queued_indicator(&app.queued) {
            // IE2: QUEUED hint. Dim italic so it doesn't compete visually
            // with the input bar. See `docs/TODO.md` § TUI ergonomics § IE2.
            let line = Paragraph::new(Line::from(Span::styled(
                text,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            frame.render_widget(line, chunks[2]);
        }
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

    // The "Always allow / Always deny" sub-prompt is a transient modal
    // that floats above whichever overlay opened it (Ask modal or
    // /permissions). When it's open, every key routes to its handler
    // (see events::handle_key); rendering it last ensures it visibly
    // covers the overlay underneath so the operator knows they're
    // interacting with the sub-prompt, not the original modal.
    if let Some(sp) = app.always_subprompt.as_ref() {
        let area = super::overlay::centered_rect(70, 60, frame.area());
        frame.render_widget(ratatui::widgets::Clear, area);
        let (tool, input_excerpt) = match app.ask_modal.as_ref() {
            Some(req) => (req.tool_name.as_str(), req.input_summary.as_str()),
            None => ("(rule editor)", ""),
        };
        super::ask::render_always_subprompt(frame, area, sp, tool, input_excerpt);
    }
}

/// Float a completion menu directly above the input area. Capped at 8 rows;
/// the highlighted item gets a cyan background. Drawn with `Clear` so it
/// overwrites the transcript rows it floats over.
pub(crate) fn render_input_menu(
    frame: &mut ratatui::Frame<'_>,
    input_area: Rect,
    menu: &input::MenuState,
) {
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
pub(crate) fn format_tool_input(input: &str, max_chars: usize) -> String {
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
pub(crate) fn wrap_lines_to_width<'a>(lines: Vec<Line<'a>>, width: u16) -> Vec<Line<'a>> {
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
pub(crate) fn render_transcript(app: &App) -> Vec<Line<'_>> {
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
pub(crate) fn format_bytes(n: u64) -> String {
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
pub(crate) fn format_cache_suffix(cache_read: Option<u32>, cache_creation: Option<u32>) -> String {
    let r = cache_read.unwrap_or(0);
    let c = cache_creation.unwrap_or(0);
    match (r, c) {
        (0, 0) => String::new(),
        (r, 0) => format!(" ({r} cached)"),
        (0, c) => format!(" ({c} cache write)"),
        (r, c) => format!(" ({r} cached, {c} write)"),
    }
}

/// Format the running-activity label, surfacing a "no tokens for Ns" hint
/// when the SSE stream has been idle for ≥10s and no tool is running.
///
/// Plan A T12: gives operators a visible signal that the model went quiet
/// instead of leaving the spinner alone, which is indistinguishable from a
/// normal slow turn.
#[must_use]
pub(crate) fn format_spinner_cell(
    active_tools: bool,
    last_delta_at: std::time::Instant,
    now: std::time::Instant,
) -> String {
    let elapsed = now.duration_since(last_delta_at);
    if !active_tools && elapsed >= std::time::Duration::from_secs(3) {
        let secs = elapsed.as_secs();
        if secs >= 10 {
            return format!("Thinking\u{2026} (no tokens for {secs}s)");
        }
        return "Thinking\u{2026}".to_string();
    }
    "Thinking\u{2026}".to_string()
}

pub(crate) fn render_status(app: &App) -> Line<'static> {
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
        // Plan A T12: surface a "no tokens for Ns" hint when the SSE stream
        // has been silent past the stall threshold. `active_tools` is derived
        // from the running activity so tool dispatches don't trip the hint.
        let active_tools = matches!(
            running.activity,
            super::app::Activity::DispatchingTool { .. }
        );
        let hint = format_spinner_cell(active_tools, app.last_delta_at, std::time::Instant::now());
        let label = if hint.contains("no tokens") {
            // Replace the default "Thinking…" label with the hint while
            // preserving the regular label for non-stalled states.
            hint
        } else {
            running.activity.label()
        };
        format!(" \u{00B7} {spinner} {label} ({secs}s)")
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

    let main_text = format!(
        " {cwd} \u{00B7} {provider} {model}{session_part}{plan_part}{perm_mode_part}{overlay_part}{running_part}{context_part}"
    );
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        main_text,
        Style::default().bg(Color::DarkGray).fg(Color::White),
    )];
    // Bypass-latch chip: visible whenever the latch is set, regardless of
    // the current PermissionMode. Rendered in bold red so it is never missed.
    if app.bypass_latch {
        spans.push(Span::styled(
            " \u{26A0} bypass latched (Ctrl+Shift+B to drop) ",
            Style::default()
                .fg(Color::Red)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// IE3: walk every cell in `area` of `buf` and record `(y, x) → first
/// char of cell.symbol()` into `map`. Called by the renderer after
/// the transcript is drawn, before any highlight overlay is applied,
/// so subsequent mouse-up selection extracts the user-visible text.
/// `map` is cleared first so each frame starts fresh. Cells whose
/// symbol is empty or pure whitespace at the start of a run are still
/// recorded (so selection across padding produces coherent output).
/// See `docs/TODO.md` § TUI ergonomics § IE3.
pub(crate) fn record_transcript_cells_into_position_map(
    buf: &ratatui::buffer::Buffer,
    area: Rect,
    map: &mut super::mouse_select::PositionMap,
) {
    map.clear();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell((x, y))
                && let Some(ch) = cell.symbol().chars().next()
            {
                map.record(y, x, ch);
            }
        }
    }
}

/// IE3: overlay a background-colour highlight on the cells in
/// `selection` (clipped to `area`). Called by the renderer after the
/// transcript draw + position-map population so the highlight composites
/// over the original glyph styles. Multi-row selections highlight
/// from `start.col` to end-of-area on the first row, full row width
/// on intermediate rows, and from area start to `end.col` on the
/// last row. Order of endpoints is normalised. No-op for selections
/// fully outside `area`. See `docs/TODO.md` § TUI ergonomics § IE3.
pub(crate) fn apply_selection_highlight(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    selection: (super::mouse_select::Cell, super::mouse_select::Cell),
    style: Style,
) {
    let (a, b) = if (selection.0.row, selection.0.col) <= (selection.1.row, selection.1.col) {
        (selection.0, selection.1)
    } else {
        (selection.1, selection.0)
    };
    let area_end_x = area.x.saturating_add(area.width);
    let area_end_y = area.y.saturating_add(area.height);
    for y in a.row..=b.row {
        if y < area.y || y >= area_end_y {
            continue;
        }
        let lo = if y == a.row { a.col } else { area.x };
        let hi = if y == b.row {
            b.col
        } else {
            area_end_x.saturating_sub(1)
        };
        let lo = lo.max(area.x);
        let hi = hi.min(area_end_x.saturating_sub(1));
        for x in lo..=hi {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(style);
            }
        }
    }
}

/// IE2: format the QUEUED indicator shown in the toast strip when
/// `app.queued` is non-empty and no toast is currently active. Returns
/// `None` for an empty queue, otherwise a single-line preview capped
/// at `QUEUED_PREVIEW_CHARS` characters, with a `(+N more)` suffix
/// when more than one message is queued. Caller wraps the result in
/// a styled `Paragraph`. See `docs/TODO.md` § TUI ergonomics § IE2.
const QUEUED_PREVIEW_CHARS: usize = 48;
pub(crate) fn format_queued_indicator(
    queue: &std::collections::VecDeque<String>,
) -> Option<String> {
    let front = queue.front()?;
    let preview: String = front.chars().take(QUEUED_PREVIEW_CHARS).collect();
    let suffix = if queue.len() > 1 {
        format!(" (+{} more)", queue.len() - 1)
    } else {
        String::new()
    };
    Some(format!("QUEUED: {preview}{suffix}"))
}

#[cfg(test)]
mod mouse_select_render_tests {
    use super::*;
    use crate::tui::mouse_select::{Cell as SelCell, PositionMap};
    use ratatui::buffer::Buffer;

    fn buf_from_lines(lines: &[&str]) -> Buffer {
        let width = u16::try_from(lines.iter().map(|l| l.chars().count()).max().unwrap_or(0))
            .expect("width fits u16");
        let height = u16::try_from(lines.len()).expect("height fits u16");
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        for (y, line) in lines.iter().enumerate() {
            for (x, ch) in line.chars().enumerate() {
                let mut s = [0u8; 4];
                let s = ch.encode_utf8(&mut s).to_string();
                if let Some(cell) =
                    buf.cell_mut((u16::try_from(x).unwrap(), u16::try_from(y).unwrap()))
                {
                    cell.set_symbol(&s);
                }
            }
        }
        buf
    }

    #[test]
    fn record_transcript_cells_round_trips_single_line() {
        let buf = buf_from_lines(&["hello"]);
        let area = Rect::new(0, 0, 5, 1);
        let mut map = PositionMap::new();
        record_transcript_cells_into_position_map(&buf, area, &mut map);
        // Buffer is at (0, 0) so map records (row=0, col=0..4).
        let extracted = map.extract_range(SelCell::new(0, 0), SelCell::new(0, 4));
        assert_eq!(extracted, "hello");
    }

    #[test]
    fn record_transcript_cells_only_within_area() {
        let buf = buf_from_lines(&["abcde", "fghij", "klmno"]);
        // Only the middle row should be recorded.
        let area = Rect::new(0, 1, 5, 1);
        let mut map = PositionMap::new();
        record_transcript_cells_into_position_map(&buf, area, &mut map);
        // Cells outside area not recorded.
        assert!(map.get(0, 0).is_none());
        assert!(map.get(2, 0).is_none());
        // Cells inside area recorded.
        assert_eq!(map.get(1, 0), Some('f'));
        assert_eq!(map.get(1, 4), Some('j'));
    }

    #[test]
    fn record_clears_map_each_call() {
        let buf1 = buf_from_lines(&["aaa"]);
        let buf2 = buf_from_lines(&["bbb"]);
        let area = Rect::new(0, 0, 3, 1);
        let mut map = PositionMap::new();
        record_transcript_cells_into_position_map(&buf1, area, &mut map);
        assert_eq!(map.get(0, 0), Some('a'));
        record_transcript_cells_into_position_map(&buf2, area, &mut map);
        assert_eq!(map.get(0, 0), Some('b'));
    }

    #[test]
    fn apply_selection_highlight_styles_cells_in_single_row() {
        let mut buf = buf_from_lines(&["hello world"]);
        let area = Rect::new(0, 0, 11, 1);
        let style = Style::default().bg(Color::DarkGray);
        apply_selection_highlight(
            &mut buf,
            area,
            (SelCell::new(0, 0), SelCell::new(0, 4)),
            style,
        );
        for x in 0..=4 {
            assert_eq!(buf.cell((x, 0)).unwrap().style().bg, Some(Color::DarkGray));
        }
        // Outside the range — no highlight.
        assert!(
            buf.cell((5, 0)).unwrap().style().bg.is_none()
                || buf.cell((5, 0)).unwrap().style().bg != Some(Color::DarkGray)
        );
    }

    #[test]
    fn apply_selection_highlight_normalises_reversed_endpoints() {
        let mut buf = buf_from_lines(&["abcde"]);
        let area = Rect::new(0, 0, 5, 1);
        let style = Style::default().bg(Color::Red);
        // Reversed: (0,3) -> (0,1) should highlight cols 1..=3.
        apply_selection_highlight(
            &mut buf,
            area,
            (SelCell::new(0, 3), SelCell::new(0, 1)),
            style,
        );
        for x in 1..=3 {
            assert_eq!(buf.cell((x, 0)).unwrap().style().bg, Some(Color::Red));
        }
    }

    #[test]
    fn apply_selection_highlight_clips_to_area() {
        let mut buf = buf_from_lines(&["abcde", "fghij"]);
        // Area is only the second row.
        let area = Rect::new(0, 1, 5, 1);
        let style = Style::default().bg(Color::Green);
        // Selection spans both rows; only the in-area cells get styled.
        apply_selection_highlight(
            &mut buf,
            area,
            (SelCell::new(0, 0), SelCell::new(1, 4)),
            style,
        );
        // Row 0 untouched.
        for x in 0..5 {
            assert_ne!(buf.cell((x, 0)).unwrap().style().bg, Some(Color::Green));
        }
        // Row 1 fully highlighted.
        for x in 0..5 {
            assert_eq!(buf.cell((x, 1)).unwrap().style().bg, Some(Color::Green));
        }
    }
}

#[cfg(test)]
mod queued_indicator_tests {
    use super::*;
    use std::collections::VecDeque;

    /// IE2 render (RED): the QUEUED indicator string is None for an
    /// empty queue and a `QUEUED: <preview>` for a non-empty one.
    /// Long previews truncate to a fixed char cap; multi-item queues
    /// append ` (+N more)`.
    #[test]
    fn format_queued_indicator_none_when_empty() {
        let q: VecDeque<String> = VecDeque::new();
        assert!(format_queued_indicator(&q).is_none());
    }

    #[test]
    fn format_queued_indicator_single_item() {
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back("hello world".into());
        assert_eq!(
            format_queued_indicator(&q).as_deref(),
            Some("QUEUED: hello world"),
        );
    }

    #[test]
    fn format_queued_indicator_appends_count_suffix_when_many() {
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back("first".into());
        q.push_back("second".into());
        q.push_back("third".into());
        assert_eq!(
            format_queued_indicator(&q).as_deref(),
            Some("QUEUED: first (+2 more)"),
        );
    }

    #[test]
    fn format_queued_indicator_truncates_long_preview() {
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back("x".repeat(120));
        let out = format_queued_indicator(&q).expect("non-empty");
        // "QUEUED: " (8) + 48 x's = 56 chars
        assert_eq!(out.chars().count(), 8 + 48);
        assert!(out.starts_with("QUEUED: xxxxx"));
    }
}

#[cfg(test)]
mod stalled_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn render_spinner_stalled_when_idle_over_3s_no_tools() {
        let now = Instant::now();
        let last_delta = now
            .checked_sub(Duration::from_secs(12))
            .expect("monotonic clock 12s back");
        let label = format_spinner_cell(false, last_delta, now);
        assert!(label.contains("no tokens for 12s"));
    }

    #[test]
    fn render_spinner_normal_under_3s() {
        let now = Instant::now();
        let last_delta = now
            .checked_sub(Duration::from_secs(1))
            .expect("monotonic clock 1s back");
        let label = format_spinner_cell(false, last_delta, now);
        assert!(!label.contains("no tokens"));
    }

    #[test]
    fn render_spinner_normal_when_tools_active() {
        let now = Instant::now();
        let last_delta = now
            .checked_sub(Duration::from_secs(30))
            .expect("monotonic clock 30s back");
        let label = format_spinner_cell(true, last_delta, now);
        assert!(!label.contains("no tokens"));
    }
}
