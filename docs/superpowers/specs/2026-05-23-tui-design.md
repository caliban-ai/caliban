# TUI (Terminal UI) · Design

- **Date:** 2026-05-23
- **Status:** Draft
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** Layer 1 (B+C+D), Layer 4 CLI, REPL+Sessions sub-project
- **Next sub-project:** TBD — memory, MCP client, or model-router

## Goals

Replace the rustyline-based REPL with a proper terminal UI that mirrors the Claude Code experience: a dedicated input area at the bottom, a status bar above it showing the current working directory + agent context, and a scrolling output region for the conversation.

**Acceptance:** Running `caliban` (no prompt, TTY) opens a TUI:

```
┌────────────────────────────────────────────────────────────────┐
│ user: What's in README.md?                                     │
│                                                                │
│ 🔧 Read({"path":"README.md"})                                  │
│    → → Read README.md, lines 1-83 of 83                        │
│                                                                │
│ assistant: It's a Rust agent harness called caliban that…      │
│                                                                │
│ [caliban: 2 turns · 132↑ 48↓ tokens]                           │
│                                                                │
│                                                                │
├────────────────────────────────────────────────────────────────┤
│ ~/dev/personal/iron-orrery · openai gpt-4o · session: research │
├────────────────────────────────────────────────────────────────┤
│ > █                                                            │
└────────────────────────────────────────────────────────────────┘
```

The user types into the bottom input area. Pressing Enter sends the prompt; the model streams its response into the output region above. Status bar at the bottom shows `cwd · provider model · session` by default. Ctrl+C cancels the in-flight turn (back to prompt); Ctrl+D or `/exit` exits.

## Non-goals

- Markdown/syntax-highlighted rendering of model output — plain text only for v1.
- Mouse support — keyboard-only.
- Multi-pane layouts (split views, side panel for tool history) — single output region.
- Customizable status bar widgets — fixed layout.
- File-tree explorer / autocomplete — input is plain text.
- Color themes — single hardcoded scheme.

## Architecture

### Layout (ratatui)

Three vertical regions, configured via `Layout::default().direction(Direction::Vertical)`:

1. **Output region** — flex-grow; takes all remaining height. Renders the transcript via `Paragraph` with `wrap(true)`.
2. **Status bar** — fixed 1 line tall.
3. **Input area** — fixed 1 line tall (multi-line follow-up).

```rust
Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(0),      // output
        Constraint::Length(1),   // status
        Constraint::Length(2),   // input area (border + 1 line)
    ])
    .split(frame.area());
```

### Event loop

Two concurrent event sources, multiplexed with `tokio::select!`:

1. **Terminal events** from `crossterm::event::EventStream` (async, requires `event-stream` feature).
2. **Agent stream events** from `Arc<Agent>::stream_until_done(...)` — active only while a turn is running.

```rust
loop {
    tokio::select! {
        Some(Ok(crossterm_event)) = term_events.next() => {
            handle_terminal_event(crossterm_event, &mut app).await?;
        }
        Some(agent_event) = agent_stream.as_mut().map(|s| s.next()).unwrap_or(future::pending()) => {
            handle_agent_event(agent_event, &mut app).await?;
        }
    }
    terminal.draw(|f| ui::render(f, &app))?;
    if app.should_exit { break; }
}
```

### App state

```rust
struct App {
    // Persistent state
    agent: Arc<Agent>,
    session: Option<PersistedSession>,
    store: Option<SessionStore>,
    args: Args,
    cwd: PathBuf,

    // Transcript
    transcript: Vec<TranscriptLine>,

    // Input editor
    input: String,
    cursor: usize,        // byte index into input
    history: Vec<String>, // submitted prompts in this session
    history_index: Option<usize>,  // Some(n) when browsing with arrow keys

    // Scrolling
    scroll: u16,          // lines scrolled up from bottom
    auto_scroll: bool,    // resets to true when user submits

    // Agent run state
    running: Option<RunningTurn>,

    // Lifecycle
    should_exit: bool,
}

struct RunningTurn {
    cancel: CancellationToken,
    accumulating_text_line: Option<usize>,  // index into transcript of the line being filled
    tool_inputs: HashMap<String, String>,
}

enum TranscriptLine {
    UserPrompt(String),
    AssistantText(String),  // mutable: extended as deltas arrive
    AssistantThinking(String),
    ToolCall {
        tool_use_id: String,
        name: String,
        input: String,      // accumulated from InputDelta
        result: Option<(bool, String)>,  // (is_error, summarized result), None until ToolCallEnd
    },
    UsageSummary { input_tokens: u32, output_tokens: u32, turn_count: u32 },
    Info(String),
    Error(String),
}
```

### Rendering

`ui::render(frame, app)` performs three writes per frame:

**1. Output region (Paragraph + scroll):**

Flatten `app.transcript` into `Vec<Line>` (ratatui's `text::Line`). Each `TranscriptLine` becomes one or more `Line`s with appropriate styling:

- `UserPrompt(text)` → bold "user: " prefix + text
- `AssistantText(text)` → no prefix, plain text
- `AssistantThinking(text)` → dim italic, prefixed with "(thinking) "
- `ToolCall { name, input, result }` → 🔧 prefix + name + input summary + result summary
- `UsageSummary` → dim "[caliban: ...]"
- `Info(text)` → dim "[" + text + "]"
- `Error(text)` → red "error: " + text

Wrap with `Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.scroll, 0))`.

Auto-scroll to bottom: `app.scroll` is computed each render as `total_visible_lines.saturating_sub(area.height)` when `app.auto_scroll == true`. When user presses PageUp/PageDown, `auto_scroll` becomes false until they hit End or submit a new prompt.

**2. Status bar:**

A single line with: `cwd · provider model[· session: name (turns)] [· running…]`

- cwd: `tilde-prefix-collapse` user home → `~/...`
- session info shown only if `session.is_some()`
- "running…" appears while a turn is in flight

Rendered with `Paragraph` on a styled background (subtle).

**3. Input area:**

`> {input}` with the cursor positioned at `input.char_indices().nth(cursor).map(|(i,_)| i).unwrap_or(input.len())`. ratatui doesn't position a real cursor; we either render a styled block character or call `frame.set_cursor_position(...)`.

### Terminal events

Map crossterm `KeyEvent` to `Action` enum:

| Key | Action |
|---|---|
| Char(c) | Insert c at cursor |
| Backspace | Delete char before cursor |
| Delete | Delete char at cursor |
| Left / Right | Move cursor |
| Home / End | Cursor to start / end |
| Up / Down | History navigation |
| Enter | Submit input |
| Ctrl+C | Cancel turn or clear input |
| Ctrl+D | Exit if input empty, else nothing |
| Ctrl+L | Clear transcript |
| PageUp / PageDown | Scroll output region |
| Esc | Cancel turn |

Slash commands (input starting with `/`) handled separately:

| Command | Effect |
|---|---|
| `/help` | Append help text to transcript |
| `/exit`, `/quit` | Save session (if any) + exit |
| `/clear` | Clear transcript |
| `/sessions` | List sessions in transcript |
| `/save [<name>]` | Save current session (rename if given) |
| `/usage` | Show usage in transcript |

### Streaming integration

When user submits input (non-slash):
1. Append `TranscriptLine::UserPrompt(input)` to transcript.
2. Push input to `app.history`.
3. Clear `input`, `cursor = 0`, `auto_scroll = true`.
4. Construct messages = session.messages.clone() + UserPrompt converted to Message.
5. Start `agent.stream_until_done(messages, cancel)` — store the stream + cancel token in `app.running`.

While running:
- `TurnEvent::AssistantTextDelta { text }` → find or create a `TranscriptLine::AssistantText` at the appropriate position; extend its String.
- `TurnEvent::AssistantThinkingDelta { text }` → same for AssistantThinking.
- `TurnEvent::ToolCallStart { tool_use_id, name }` → append `TranscriptLine::ToolCall { tool_use_id, name, input: "", result: None }`.
- `TurnEvent::ToolCallInputDelta` → accumulate input.
- `TurnEvent::ToolCallEnd` → update result.
- `TurnEvent::TurnEnd { turn_index, usage, .. }` → no transcript change (per-turn usage rolled into RunEnd).
- `TurnEvent::RunEnd { final_messages, total_usage, turn_count, stopped_for }` →
  - Append `TranscriptLine::UsageSummary`.
  - If session: `session.merge_run(final_messages, total_usage); store.save(&session);` and append `TranscriptLine::Info("session saved")`.
  - Clear `app.running`.

### Cancellation

Ctrl+C calls `app.running.cancel.cancel()`. The stream consumer sees `Error::Cancelled`, appends `TranscriptLine::Info("turn cancelled")`, and clears `app.running`.

### Terminal raw mode + alternate screen

On entry:
- `crossterm::terminal::enable_raw_mode()?`
- `execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?` — actually mouse capture not needed (non-goal); just alternate screen.

On exit (panic-safe via `Drop` impl):
- `execute!(stdout(), LeaveAlternateScreen)?`
- `disable_raw_mode()?`

Use a `TerminalGuard` RAII type to make sure the terminal is restored even on panic.

## Crate changes

`caliban/Cargo.toml` additions:

```toml
ratatui   = { version = "0.28", features = ["crossterm"] }
crossterm = { version = "0.28", features = ["event-stream"] }
```

ratatui 0.28+ uses crossterm 0.28; matching versions matters.

New module: `caliban/src/tui/mod.rs`, `caliban/src/tui/{app,ui,events,render}.rs` (probably split for clarity since the file gets large).

The existing `caliban/src/repl.rs` either gets DELETED (replaced by TUI) or KEPT under a `--simple-repl` flag for non-TTY-but-still-want-interactive scenarios. **Recommendation: delete it.** The single-prompt mode still works for non-TTY; the TUI replaces the rustyline REPL entirely.

If `caliban` is invoked with no prompt but stdin is NOT a TTY, error out: `"caliban: no prompt given and stdin is not a TTY; use --prompt or pass a prompt argument"`.

## Acceptance criteria

- `cargo build --bin caliban` builds with the new ratatui dep.
- `cargo test --workspace` passes (existing tests + new tests for `App` state transitions).
- Running `caliban` in a TTY opens the TUI; pressing Enter on empty input is a no-op; typing + Enter sends a prompt; the response streams into the output region above the status bar.
- `Ctrl+C` cancels mid-turn → "turn cancelled" line in transcript, prompt returns.
- `Ctrl+D` on empty input → exits cleanly (session saved if applicable), terminal restored.
- `/sessions` lists sessions in the transcript.
- Status bar shows cwd (with `~` collapse) + provider + model + session.
- Resizing the terminal works (ratatui handles this automatically).
- Existing CLI flags + single-prompt mode continue to work unchanged.
- One new ADR (0012) documenting TUI choice (ratatui, three-region layout, replacing rustyline REPL).
- README updated to reflect TUI as the default interactive mode.

## Risks

- **ratatui learning curve.** ratatui is a frame-based renderer — every frame redraws the whole UI. Different mental model from rustyline. Mitigation: keep state minimal; ratatui's `Paragraph` widget handles the heavy lifting.
- **Streaming text rendering performance.** Re-rendering the full Paragraph on every text delta is fast for short transcripts but could lag at thousands of lines. Mitigation: only re-render on actual changes (debounce delta accumulation).
- **Terminal state recovery on panic.** Forgetting to restore raw mode + leave alt screen leaves the user's terminal broken. Mitigation: `TerminalGuard` RAII; also set a `panic::set_hook` that restores before printing the panic.
- **Mouse-wheel scrolling.** If the user expects mouse scrolling to work in the output region, that needs `EnableMouseCapture` AND handler. Out of scope for v1 (keyboard only).
- **Windows terminal compatibility.** ratatui + crossterm should work on Windows Terminal / cmd / PowerShell, but the experience may differ. v1 focuses on macOS/Linux; Windows is "should work but untested."
- **Color contrast.** Default styles assume a dark terminal; on light terminals, dim text may be unreadable. Mitigation: avoid relying on dim ANSI; use simple foreground colors that work on both.

## Implementation order

1. **T.1 — Scaffold:** `caliban/src/tui/` modules; `App` state struct; basic three-region render with no agent integration; raw-mode entry/exit + RAII guard; handle terminal events (input editing, exit on Ctrl+D).
2. **T.2 — Agent integration:** Wire `stream_until_done` into the event loop; render TranscriptLines from TurnEvents; cancellation via Ctrl+C.
3. **T.3 — Status bar polish:** cwd with `~` collapse; provider/model/session display; "running…" indicator.
4. **T.4 — Slash commands + session save-on-exit:** Adapt the REPL's command handler to write to transcript instead of stdout; auto-save on exit.
5. **T.5 — Replace REPL dispatch + delete repl.rs:** Update `main.rs` to dispatch to TUI in the previous REPL-trigger conditions; remove the now-unused `repl.rs` and `rustyline` dependency.
6. **T.6 — ADR 0012 + README update.**
