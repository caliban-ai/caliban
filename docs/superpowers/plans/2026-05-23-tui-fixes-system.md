# TUI Fixes + System Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Add default system prompt + per-invocation override flags + `/system` overlay; fix streaming stalls via tick-based redraw + explicit flush + yield_now; add `--debug` logging.

**Architecture:** New `caliban/src/system_prompt.rs` module builds the default prompt and resolves precedence (override flags vs. default). TUI event loop gains a `tokio::time::interval` tick arm. `tracing_subscriber` installs a file appender when `--debug` is set.

**Tech Stack:** tracing_subscriber 0.3 (fmt + env-filter features).

**Spec:** [`docs/superpowers/specs/2026-05-23-tui-fixes-system-design.md`](../specs/2026-05-23-tui-fixes-system-design.md)

---

## Task V.1: System prompt module + CLI flags + integration

**Files:**
- Create: `caliban/src/system_prompt.rs`
- Modify: `caliban/src/main.rs`
- Modify: `caliban/Cargo.toml` (no new deps needed for V.1)

- [ ] **Step 1: `caliban/src/system_prompt.rs`**

```rust
//! Default system prompt + override resolution.

use std::path::Path;

use anyhow::Context;

/// Build the default system prompt from current state.
#[must_use]
pub fn build_default(cwd: &Path, tool_names: &[&str], no_tools: bool) -> String {
    let cwd_str = cwd.display();

    let tools_section = if no_tools {
        "Tools are disabled for this session.".to_string()
    } else {
        let mut s = String::from("You have access to these tools:\n");
        for name in tool_names {
            s.push_str(&match *name {
                "Read" => "- Read(path, [limit, offset]) — read text files (max 5MB, line-indexed)\n",
                "Write" => "- Write(path, content) — create or overwrite files (auto-creates parents)\n",
                "Edit" => "- Edit(path, old_string, new_string, [replace_all]) — string replacement in files\n",
                "Bash" => "- Bash(command, [timeout_seconds, cwd]) — execute /bin/sh -c \"...\"; captures stdout/stderr\n",
                "Glob" => "- Glob(pattern, [path]) — find files matching a glob (.gitignore-aware)\n",
                "Grep" => "- Grep(pattern, [path, include, max_matches]) — ripgrep-style content search\n",
                other => format!("- {other}\n"),
            });
        }
        s
    };

    format!(
        "You are caliban, an agentic command-line assistant running inside the caliban harness \
        (a from-scratch Rust replacement for Claude Code).\n\
        \n\
        You are operating in the following directory:\n  {cwd_str}\n\
        \n\
        {tools_section}\
        \n\
        Conventions:\n\
        - Use tools when needed; don't claim to have read files you haven't actually Read.\n\
        - File paths can be relative to the working directory above, or absolute.\n\
        - Bash commands run with /bin/sh -c and timeout after 60s by default.\n\
        - Output is rendered in a terminal UI; prefer concise responses with code blocks for \
        multi-line content rather than long prose paragraphs.\n\
        - When the user asks you to modify a file, Read it first so your edits are accurate.\n\
        \n\
        Ask before destructive operations (rm -rf, force-pushing git, dropping database tables, etc.).\n"
    )
}

/// Resolve the system prompt to use based on CLI args.
///
/// Precedence: `--system` > `--system-file` > default. `--no-system` returns Ok(None).
///
/// # Errors
/// Returns an error if `--system-file` is given but cannot be read.
pub fn resolve(
    system: Option<&str>,
    system_file: Option<&Path>,
    no_system: bool,
    cwd: &Path,
    tool_names: &[&str],
    no_tools: bool,
) -> anyhow::Result<Option<String>> {
    if no_system { return Ok(None); }
    if let Some(text) = system { return Ok(Some(text.to_string())); }
    if let Some(path) = system_file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading system prompt from {}", path.display()))?;
        return Ok(Some(text));
    }
    Ok(Some(build_default(cwd, tool_names, no_tools)))
}
```

- [ ] **Step 2: Add CLI flags to `Args` in `main.rs`**

```rust
/// Override system prompt with the given text.
#[arg(long, value_name = "STRING", conflicts_with_all = ["system_file", "no_system"])]
pub(crate) system: Option<String>,

/// Override system prompt with the contents of a file.
#[arg(long, value_name = "PATH", conflicts_with_all = ["system", "no_system"])]
pub(crate) system_file: Option<PathBuf>,

/// Run with no system prompt (disables the default).
#[arg(long, conflicts_with_all = ["system", "system_file"])]
pub(crate) no_system: bool,
```

- [ ] **Step 3: Add `mod system_prompt;` to main.rs**

- [ ] **Step 4: Wire into main flow**

After agent + (optional) session loading, but BEFORE running:

```rust
let tool_names: Vec<&str> = agent.tools().names().collect();

let system_prompt = system_prompt::resolve(
    args.system.as_deref(),
    args.system_file.as_deref(),
    args.no_system,
    &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    &tool_names,
    args.no_tools,
)?;

// If creating a NEW session (or running ephemeral with no prior history),
// prepend the system prompt as message[0].
if let Some(sess) = session.as_mut() {
    // Fresh session: messages is empty, insert system at start.
    if sess.messages.is_empty() {
        if let Some(prompt) = &system_prompt {
            sess.messages.push(caliban_provider::Message::system_text(prompt.clone()));
        }
    }
    // Existing session: do NOT touch — system prompt is already in messages[0].
}

// For ephemeral (no session), prepend system as part of the messages built in single-prompt mode.
// In `run_and_render` and `tui::run`, prepend system to messages on the first turn.
```

The cleanest fix: pass `system_prompt: Option<String>` into both single-prompt code path AND `tui::run`, where the prompt is prepended to messages for the first turn IF there are no Role::System messages already.

Update `tui::run` signature and `App::new` to accept the system prompt; store on `App`. When building messages for a turn:

```rust
let mut messages: Vec<Message> = app.session.as_ref().map(|s| s.messages.clone()).unwrap_or_default();
let has_system = messages.first().map_or(false, |m| m.role == caliban_provider::Role::System);
if !has_system {
    if let Some(prompt) = &app.system_prompt {
        messages.insert(0, Message::system_text(prompt.clone()));
    }
}
messages.push(Message::user_text(prompt_text));
```

Same in `run_and_render` for the single-prompt path.

- [ ] **Step 5: Build + test + commit**

```bash
cargo build  --bin caliban
cargo test   -p caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(cli): default system prompt + --system / --system-file / --no-system

Adds caliban/src/system_prompt.rs building a default prompt that
includes caliban's identity, the current working directory, the
registered tools, and basic operating conventions. Inserted as the
first Role::System message at session creation (or prepended each
invocation in ephemeral mode).

Three new flags allow overrides: --system <TEXT> sets a literal,
--system-file <PATH> reads from a file, --no-system disables the
prompt entirely. Mutually exclusive via clap.

The default tool list is built from the actually-registered tool
names, so --no-tools or future tool plugins reflect correctly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task V.2: Tick-based redraw + flush + yield_now

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Add tick interval to the select! arms**

In `tui::run`:

```rust
use std::time::Duration;

let mut tick = tokio::time::interval(Duration::from_millis(50));
tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
```

Then in the select!:

```rust
tokio::select! {
    term_event = events.next() => { /* existing */ }
    agent_event = async { ... } => { /* existing */ }
    _ = tick.tick() => {
        // No-op; the loop will redraw on next iteration.
    }
}
```

- [ ] **Step 2: Explicit flush after each draw**

```rust
guard.terminal().draw(|frame| render(frame, &app))?;
use std::io::Write;
std::io::stdout().flush().ok();
```

- [ ] **Step 3: yield_now after each select! iteration**

After the select! block, before looping:

```rust
tokio::task::yield_now().await;
```

- [ ] **Step 4: Build + manual test + commit**

```bash
cargo build  --bin caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

Manual: with an API key, run `./target/debug/caliban` and ask the model something. Stream should render smoothly. If it stalls, behavior should be visibly different (status bar should at minimum still update).

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
fix(tui): tick-based redraw + explicit flush + yield_now

Three belt-and-suspenders fixes for occasional streaming stalls:

1. Tick interval at 50ms (20 Hz) added to the event loop's select!.
   Even with no terminal or agent events, the loop iterates and
   redraws. This prevents stalls from missed wakeups.
2. std::io::stdout().flush() after each terminal.draw() call.
   Ratatui's backend flushes internally but this is a no-cost
   belt-and-suspenders against any platform-specific buffering.
3. tokio::task::yield_now() between iterations, ensuring runtime
   fairness so neither the EventStream task nor the HTTP-streaming
   task can starve the loop.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task V.3: `/system` overlay

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Add `Overlay::System` variant**

```rust
pub(crate) enum Overlay {
    SlashHelp,
    Config,
    Mcp,
    Skills,
    System,  // NEW
}
```

Update `title()` and `short_name()`:

```rust
Self::System => "System Prompt",
// short_name:
Self::System => "system",
```

- [ ] **Step 2: Add `system_lines(app: &App) -> Vec<Line<'_>>`**

```rust
fn system_lines(app: &App) -> Vec<Line<'static>> {
    let system_text = app.session.as_ref()
        .and_then(|s| s.messages.iter().find(|m| m.role == caliban_provider::Role::System))
        .and_then(|m| m.content.iter().find_map(|c| match c {
            caliban_provider::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        }));

    let mut out = vec![Line::raw("")];
    match system_text {
        Some(text) => {
            for line in text.lines() {
                out.push(Line::raw(line.to_string()));
            }
        }
        None => {
            out.push(Line::raw("(no system prompt — use --system or --system-file to set one)"));
        }
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        "  Press q or Esc to close. Edit via --system-file or by editing the session JSON.",
        Style::default().add_modifier(Modifier::DIM),
    ));
    out
}
```

- [ ] **Step 3: Wire into `render_overlay` dispatch**

In the `match overlay` block in `render_overlay`, add:

```rust
Overlay::System => system_lines(app),
```

- [ ] **Step 4: Wire `/system` slash command**

In `handle_slash_command`:

```rust
"/system" => {
    app.view = ViewState::Overlay(Overlay::System);
}
```

- [ ] **Step 5: Update `/help` overlay content**

In `slash_help_lines`, add the `/system` row alongside the existing slash commands.

- [ ] **Step 6: Build + commit**

```bash
cargo build  --bin caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

```bash
git add caliban/
git commit -m "feat(tui): /system overlay (view current system prompt)

Adds Overlay::System and the /system slash command. Renders the
first Role::System message from the current session, or '(no system
prompt)' if absent. Read-only; footer hints how to edit (via
--system-file flag or by editing the session JSON).

The /help overlay's command list now includes /system.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task V.4: Debug logging

**Files:**
- Modify: `caliban/Cargo.toml`
- Modify: `caliban/src/main.rs`
- Modify: `caliban/src/tui.rs`

- [ ] **Step 1: Add `tracing-subscriber` to deps**

```toml
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }
```

- [ ] **Step 2: Add `--debug` flag**

```rust
/// Append-log events + draws to ~/.cache/caliban/debug.log
#[arg(long)]
pub(crate) debug: bool,
```

- [ ] **Step 3: Install tracing subscriber at startup**

In `main`, before any other setup:

```rust
let debug = args.debug || std::env::var("CALIBAN_DEBUG").is_ok();
if debug {
    let log_path = dirs::cache_dir()
        .map(|d| d.join("caliban").join("debug.log"));
    if let Some(path) = log_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use tracing_subscriber::fmt;
            use tracing_subscriber::layer::SubscriberExt;
            use tracing_subscriber::util::SubscriberInitExt;
            let layer = fmt::layer().with_writer(std::sync::Mutex::new(file)).with_ansi(false);
            tracing_subscriber::registry().with(layer).init();
            tracing::info!("caliban debug logging started");
        }
    }
}
```

- [ ] **Step 4: Add tracing calls in `tui.rs`**

At the top of `handle_event`:

```rust
tracing::trace!(?event, "term event");
```

In `handle_agent_event`:

```rust
tracing::debug!(?evt, "agent event");
```

In `tui::run`'s loop, after the draw:

```rust
tracing::trace!("draw");
```

Use `tracing` lightly — too many trace calls per loop iter will spam the log.

- [ ] **Step 5: Build + manual test + commit**

```bash
cargo build  --bin caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

Manual: `CALIBAN_DEBUG=1 ./target/debug/caliban` → in another terminal: `tail -f ~/.cache/caliban/debug.log`. Should see entries.

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(tui): --debug flag + tracing-subscriber file appender

When --debug or CALIBAN_DEBUG=1 is set, caliban installs a
tracing-subscriber writing to ~/.cache/caliban/debug.log. Logs each
terminal event, each agent stream event, each draw, and errors.
Useful for diagnosing TUI stalls.

No overhead when debug is off (subscriber not installed; tracing
macros are no-ops).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task V.5: ADR 0014 + README

- Create `docs/adr/0014-system-prompt-and-tui-fixes.md` capturing the system-prompt-at-creation-time persistence rule + the tick-based redraw fix + the debug-log behind env-var.
- Append to `docs/adr/README.md`.
- Update root `README.md`:
  - In the "Interactive TUI" section, note the new flags: `--system`, `--system-file`, `--no-system`, `--debug`.
  - Add `/system` to the slash-command list.
  - Brief one-line note about default system prompt: "By default, caliban builds a system prompt describing itself, the cwd, and registered tools; override with --system, load from --system-file, or disable with --no-system."

Commit: `docs: ADR 0014 + README for system prompt + TUI fixes`.

---

## Self-Review

V.1 (system prompt) is the biggest task — new file, new flags, integration in two code paths (single-prompt + TUI). V.2 (tick fix) is the most important behaviorally. V.3 (overlay) is trivial given the overlay infrastructure exists. V.4 (debug) is small. V.5 docs.

Type consistency: `system_prompt::resolve` returns `Option<String>`; flows into `app.system_prompt: Option<String>` (added to App in V.1). The TUI's per-turn message construction reads from `app.system_prompt`.

Spec coverage: all four bullets from acceptance criteria are covered.
