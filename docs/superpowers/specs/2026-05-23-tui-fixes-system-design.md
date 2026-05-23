# TUI Fixes + System Prompt · Design

- **Date:** 2026-05-23
- **Status:** Draft
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** TUI overlays sub-project

## Goals

Address two real issues the user hit during actual use:

1. **The model doesn't know it's running in caliban.** Default system prompt should auto-include identity, working directory, available tools, and basic operating conventions. User can override via `--system` / `--system-file` / `--no-system`. A `/system` slash command opens an overlay showing the current system prompt.

2. **Streaming sometimes stalls until the user types.** The TUI's draw is event-driven; if the agent stream's events don't reliably wake the select! loop, frames lag. Belt-and-suspenders fix:
   - Add a periodic tick (~50ms) to the select! so the loop redraws even if no event arrives.
   - Force `flush()` after each `terminal.draw()`.
   - Behind `CALIBAN_DEBUG=1` (or `--debug`), append-log every event + draw to `~/.cache/caliban/debug.log` so the next stall can be diagnosed from a real trace.
   - Use `tokio::task::yield_now()` after each event-handler invocation to keep the runtime fair.

## Non-goals

- Markdown rendering of the system prompt in the overlay — plain text.
- Editing the system prompt from within the TUI — read-only for v1. Use `--system` flag or edit the session JSON file.
- Rewriting agent-core's stream — the fixes live in the CLI/TUI layer.
- A general "settings file" — config still comes from CLI flags + env vars.

## Default system prompt

Auto-built at runtime from current state:

```
You are caliban, an agentic command-line assistant running inside the
caliban harness (a from-scratch Rust replacement for Claude Code).

You are operating in the following directory:
  /Users/johnford2002/dev/personal/iron-orrery

You have access to these tools:
- Read(path, [limit, offset]) — read text files (max 5MB, line-indexed)
- Write(path, content) — create or overwrite files (auto-creates parents)
- Edit(path, old_string, new_string, [replace_all]) — string replacement in files
- Bash(command, [timeout_seconds, cwd]) — execute /bin/sh -c "..."; captures stdout/stderr
- Glob(pattern, [path]) — find files matching a glob (.gitignore-aware)
- Grep(pattern, [path, include, max_matches]) — ripgrep-style content search

Conventions:
- Use tools when needed; don't claim to have read files you haven't actually Read.
- File paths can be relative to the working directory above, or absolute.
- Bash commands run with /bin/sh -c and timeout after 60s by default.
- Output is rendered in a terminal UI; prefer concise responses with code blocks
  for multi-line content rather than long prose paragraphs.
- When the user asks you to modify a file, Read it first so your edits are
  accurate.

Ask before destructive operations (rm -rf, force-pushing git, dropping
database tables, etc.).
```

Dynamic substitutions:
- `{cwd}` → actual cwd (from `app.cwd`)
- Tool list → built from `app.agent.tools().names()` so disabled tools don't appear

If `--no-tools`, the tool list section is replaced with: `Tools are disabled for this session.`

### Override mechanisms

- `--system <text>` — replace the entire default with the given text
- `--system-file <path>` — read system prompt from a file
- `--no-system` — no system prompt at all
- (No CLI flag) — use the auto-built default

Mutually exclusive: clap enforces at most one of the three.

### Persistence

System prompt is the FIRST message in `session.messages` with `Role::System`. When a session is loaded, the existing system prompt is used (no override from current `--system`). When a NEW session is created (no prior history), the system prompt is inserted at creation time.

For the ephemeral mode (no `--session`), the system prompt is prepended to messages on every invocation.

## /system overlay

New `Overlay::System` variant. Renders the current session's system prompt (or the ephemeral session's prompt) as `Paragraph` with wrap. Header: "System Prompt". Footer: "Press q or Esc to close. Edit via --system-file or by editing the session JSON."

If the session has no system message, content is `"(no system prompt — use --system or --system-file to set one)"`.

## Streaming stall fix

### Tick-based redraw

Add `tokio::time::interval(Duration::from_millis(50))` to the select! arms:

```rust
let mut tick = tokio::time::interval(Duration::from_millis(50));
tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

loop {
    guard.terminal().draw(|frame| render(frame, &app))?;
    std::io::stdout().flush().ok();  // belt-and-suspenders flush
    if app.should_exit { break; }

    tokio::select! {
        term = events.next() => handle_term(...),
        agent = ... => handle_agent(...),
        _ = tick.tick() => {
            // No-op; loop iterates and redraws.
        }
    }
    tokio::task::yield_now().await;  // fairness
}
```

A 50ms tick = 20 Hz redraws. Even with no events, the screen updates 20x/sec. With events, redraws can be far more frequent.

### Explicit flush

`std::io::stdout().flush()` after `terminal.draw()`. Ratatui SHOULD flush via its backend, but this is a no-cost belt-and-suspenders.

### yield_now

`tokio::task::yield_now().await` between iterations. This ensures the runtime fairly schedules other tasks (the agent's HTTP-streaming task, the EventStream's polling task). If our event-handling code accidentally does too much work, this prevents starvation.

## Diagnostic logging

When `CALIBAN_DEBUG=1` (env var) OR `--debug` is set:
- Open `~/.cache/caliban/debug.log` in append mode at startup
- Log each `TurnEvent` received (timestamp + variant + relevant fields)
- Log each terminal `KeyEvent` (timestamp + key + modifiers)
- Log each `terminal.draw` call (timestamp)
- Log errors with full context

Use `tracing` (already in workspace deps) with a `tracing_subscriber::fmt::Layer` writing to the file. Format: one JSON-line per event (or pretty for readability — go with pretty since this is for human reading).

When debug is off, no overhead (tracing::Layer not installed).

The user can `tail -f ~/.cache/caliban/debug.log` while running caliban to watch what's happening.

## Crate changes

`caliban/Cargo.toml` additions:

```toml
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }
```

(Already have `tracing` via workspace deps.)

`caliban/src/main.rs` additions:
- `--system <STRING>` flag
- `--system-file <PATH>` flag
- `--no-system` flag
- `--debug` flag

Validation: exactly one of `--system`, `--system-file`, `--no-system` may be set, else clap rejects.

`caliban/src/system_prompt.rs` (new):
- `fn build_default(cwd: &Path, tool_names: &[&str]) -> String`
- `fn resolve(args: &Args, cwd: &Path, tool_names: &[&str]) -> Result<Option<String>>` — returns the system prompt to use (or None if `--no-system`)

`caliban/src/tui.rs` modifications:
- Add `Overlay::System` variant
- Add `system_lines(app) -> Vec<Line<'static>>` overlay-content function
- Add `/system` slash command
- Add the tick-based redraw + flush + yield_now

`caliban/src/main.rs` flow:
- Resolve system prompt (None | Some(text))
- After building the agent + (optional) loading session:
  - If session is fresh (no messages) or ephemeral: prepend `Message::system_text(prompt)` to messages
  - If session has prior messages: leave system prompt as-is (it's already there from initial creation)

## Acceptance criteria

- `caliban "what's in README.md?"` invokes the agent with a system prompt that describes caliban + tools + cwd.
- Resulting model responses reference caliban / the cwd / the tool names contextually (i.e., the model has been informed).
- `caliban --no-system "..."` runs with no system prompt.
- `caliban --system "You are a pirate." "..."` uses the override.
- `caliban --system-file /path/to/prompt.txt "..."` uses the file content.
- `caliban --session foo` (new session) inserts the default system prompt as message[0].
- `caliban --session foo` (existing session) preserves the session's stored system prompt — does NOT swap in the current default.
- `/system` overlay shows the current prompt.
- `caliban --debug` logs events + draws to `~/.cache/caliban/debug.log`.
- `cargo build --bin caliban && cargo test --workspace` passes.
- TUI tick is in place; even without agent activity, the status bar's "running…" indicator etc. would update at 20 Hz.
- One new ADR (0014) covering: system-prompt-on-creation persistence model, tick-based-redraw architectural fix, debug-log under env var.

## Implementation tasks

- **V.1** — system_prompt.rs + CLI flags + main.rs integration (build default, resolve, prepend to messages on creation).
- **V.2** — Tick-based redraw + explicit flush + yield_now in tui.rs (the streaming fix).
- **V.3** — `/system` overlay (variant, slash command, content function).
- **V.4** — Debug logging behind `--debug` / `CALIBAN_DEBUG=1`.
- **V.5** — ADR 0014 + README update.

## Risks

- **System prompt persistence across model switches.** If user starts a session with `claude-3-5-sonnet` and the default prompt mentions Claude conventions, then switches to `gpt-4o`, the session's stored system prompt still says Claude things. Acceptable for v1; `/system` overlay lets users see the discrepancy. A future enhancement could auto-refresh the system prompt when the model changes.
- **System prompt grows with tool count.** Listing all tools inline makes the prompt longer with each new tool. For now this is fine (6 tools = ~10 lines). When MCP/skills land, we'd want to summarize categories rather than enumerate.
- **Tick at 20 Hz could be wasteful.** 20 Hz with no changes means we redraw a static screen 20 times per second. ratatui's diffing means only changed cells get written, so the wire cost is zero. But CPU cost is non-trivial (the diff itself runs). 50ms is a reasonable middle ground; could go to 100ms if profiling shows it matters.
- **Tracing-subscriber adds dependency weight.** Not huge; `env-filter` and `fmt` are the minimal feature set. Worth it for the debug capability.
- **The stall might persist.** If the root cause is somewhere in agent-core's stream implementation (e.g., a missing waker registration), tick-based redraw masks the symptom but doesn't fix the underlying bug. The debug log will help identify if that's the case. Document this in the ADR.
