# TUI Overlays + Layout v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Flip TUI layout (input above status bar, bordered with horizontal rules), add overlay infrastructure, and ship four sub-menu overlays: /help, /config, /mcp (stub), /skills (stub).

**Architecture:** Five-task plan modifying `caliban/src/tui.rs`. New `ViewState`/`Overlay` enums tracked on `App`. Centered-rect helper renders overlays via ratatui's `Clear` + bordered `Block` + `Paragraph` widgets. Slash commands `/help`, `/config`, `/mcp`, `/skills` set view state; `Esc`/`q` resets to Main.

**Tech Stack:** ratatui 0.29, crossterm 0.29 (already in deps).

**Spec:** [`docs/superpowers/specs/2026-05-23-tui-overlays-design.md`](../specs/2026-05-23-tui-overlays-design.md)

---

## Task U.1: Layout v2 + input borders

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Update the layout chunks**

Find the `Layout::default().direction(...).constraints(...)` block in `render`. Replace with five rows:

```rust
let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(0),      // 0: output region (flex)
        Constraint::Length(1),   // 1: top border (horizontal rule)
        Constraint::Length(1),   // 2: input area
        Constraint::Length(1),   // 3: bottom border
        Constraint::Length(1),   // 4: status bar
    ])
    .split(frame.area());
```

- [ ] **Step 2: Render top/bottom horizontal rules**

After rendering the output paragraph and before the status, render two horizontal rules and the input area:

```rust
// chunks[0] = output (existing render unchanged)

// chunks[1] = top horizontal rule
let hrule_top = Block::default().borders(Borders::TOP).style(Style::default().fg(Color::DarkGray));
frame.render_widget(hrule_top, chunks[1]);

// chunks[2] = input
let input_line = Line::from(vec![
    Span::raw("> "),
    Span::raw(&app.input),
]);
frame.render_widget(Paragraph::new(input_line), chunks[2]);

// chunks[3] = bottom horizontal rule
let hrule_bot = Block::default().borders(Borders::TOP).style(Style::default().fg(Color::DarkGray));
frame.render_widget(hrule_bot, chunks[3]);

// chunks[4] = status bar
let status = render_status(app);
frame.render_widget(Paragraph::new(status), chunks[4]);

// Cursor position — now in chunks[2] (input area)
let prefix_cols: u16 = u16::try_from(app.input[..app.cursor].chars().count()).unwrap_or(0);
frame.set_cursor_position((chunks[2].x + 2 + prefix_cols, chunks[2].y));
```

Note: `Block::default().borders(Borders::TOP)` renders only the top edge — a single horizontal line spanning the chunk's width. Using `Borders::TOP` on a 1-row-tall chunk gives us exactly the horizontal rule we want.

- [ ] **Step 3: Build + manual test + commit**

```bash
cargo build  --bin caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

Manual: `./target/debug/caliban` in a terminal. Should now show output region → horizontal line → input prompt → horizontal line → status bar (cwd/provider/model). Cursor in input area.

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(tui): layout v2 — input above status bar with horizontal borders

Reorders the TUI's vertical chunks from [output, status, input] to
[output, top_border, input, bottom_border, status_bar]. The input
area is now bracketed by two single-row horizontal rules (Block with
Borders::TOP), giving it visual emphasis as the active region. Status
bar moves to the bottom.

Cursor positioning updated to point at the new input chunk.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task U.2: Overlay infrastructure + `/help` overlay

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Add `ViewState` and `Overlay` enums**

Near the top of `tui.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewState {
    Main,
    Overlay(Overlay),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overlay {
    SlashHelp,
    Config,
    Mcp,
    Skills,
}

impl Overlay {
    fn title(self) -> &'static str {
        match self {
            Self::SlashHelp => "Slash Commands",
            Self::Config => "Configuration",
            Self::Mcp => "MCP Servers",
            Self::Skills => "Skills",
        }
    }

    fn short_name(self) -> &'static str {
        match self {
            Self::SlashHelp => "help",
            Self::Config => "config",
            Self::Mcp => "mcp",
            Self::Skills => "skills",
        }
    }
}
```

- [ ] **Step 2: Add `view: ViewState` field to `App`**

```rust
pub(crate) struct App {
    // ... existing ...
    pub(crate) view: ViewState,
}
```

Initialize in `App::new`: `view: ViewState::Main`.

- [ ] **Step 3: Add centered-rect helper**

Bottom of `tui.rs`:

```rust
fn centered_rect(percent_x: u16, percent_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
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
```

- [ ] **Step 4: Add `render_overlay` function**

```rust
fn render_overlay(frame: &mut ratatui::Frame<'_>, app: &App, overlay: Overlay) {
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

    let area = centered_rect(80, 80, frame.area());

    // Clear the area underneath
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
    };

    let body = Paragraph::new(content_lines).wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}
```

- [ ] **Step 5: Add slash help content function**

```rust
fn slash_help_lines() -> Vec<Line<'static>> {
    let entries = [
        ("/help",          "Show this help"),
        ("/exit, /quit",   "Save session and exit"),
        ("/clear",         "Clear transcript"),
        ("/sessions",      "List saved sessions"),
        ("/save [<name>]", "Save current session (optionally rename)"),
        ("/usage",         "Show accumulated usage"),
        ("/config",        "Show active configuration"),
        ("/mcp",           "MCP server configuration (stub)"),
        ("/skills",        "Skills configuration (stub)"),
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
    out.push(Line::styled(" — Keyboard ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    out.push(Line::raw(""));
    let keys = [
        ("Enter",           "Submit prompt or slash command"),
        ("Backspace / Del", "Edit input"),
        ("Left / Right",    "Move cursor"),
        ("Up / Down",       "Navigate input history"),
        ("Home / End",      "Jump to start / end of input"),
        ("PageUp / PageDn", "Scroll transcript"),
        ("Ctrl+C",          "Cancel running turn or clear input"),
        ("Ctrl+D",          "Exit (when input is empty)"),
        ("Esc / q",         "Close this overlay (or cancel a running turn)"),
    ];
    for (k, desc) in keys {
        out.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("{k:<18}"), Style::default().fg(Color::Cyan)),
            Span::raw(desc),
        ]));
    }
    out.push(Line::raw(""));
    out.push(Line::styled("  Press q or Esc to close.", Style::default().add_modifier(Modifier::DIM)));
    out
}
```

(`config_lines`, `mcp_lines`, `skills_lines` come in U.3 and U.4 — for U.2 use placeholder bodies that just return `vec![Line::raw("coming soon")]`.)

- [ ] **Step 6: Wire overlay rendering into `render`**

At the end of `render` (after the status bar is drawn), check the view state:

```rust
if let ViewState::Overlay(o) = app.view {
    render_overlay(frame, app, o);
}
```

- [ ] **Step 7: Update `/help` slash command to open the overlay**

In `handle_slash_command`, change the `/help` branch:

```rust
"/help" => {
    app.view = ViewState::Overlay(Overlay::SlashHelp);
}
```

(Remove the existing Info-line-based help. The overlay is the new help.)

- [ ] **Step 8: Add overlay close keys**

In `handle_key`, add a top-level check:

```rust
fn handle_key(key: &KeyEvent, app: &mut App, agent_stream: &mut Option<TurnEventStream>) {
    if key.kind != KeyEventKind::Press { return; }

    // Overlay-mode key handling
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
            _ => {}  // Overlays are read-only in v1
        }
        return;
    }

    // ... existing main-view key handling continues ...
}
```

- [ ] **Step 9: Update status bar to show overlay indicator**

In `render_status`, append the overlay indicator when applicable:

```rust
let overlay_part = match app.view {
    ViewState::Overlay(o) => format!(" \u{00B7} [{} — q to close]", o.short_name()),
    ViewState::Main => String::new(),
};

let text = format!(" {cwd} \u{00B7} {provider} {model}{session_part}{overlay_part}{running_part}");
```

- [ ] **Step 10: Build + manual test + commit**

```bash
cargo build  --bin caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

Manual: `./target/debug/caliban` → type `/help` → see the help overlay. Type `q` or press Esc → return to main. Type `/help` again, then Ctrl+D — should be a no-op (overlay open). Close it, then Ctrl+D → exit.

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(tui): overlay infrastructure + /help overlay

Adds ViewState/Overlay enums tracked on App. /help now opens a centered
80%×80% overlay listing all slash commands + key bindings. Esc and q
close any overlay; main-view input is suppressed while one is open.
Status bar shows '[overlay — q to close]' indicator.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task U.3: `/config` overlay

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Implement `config_lines(app: &App) -> Vec<Line<'_>>`**

Read from `app.args` and `app.session`:

```rust
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
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| app.cwd_display());

    let tools_line = if app.args.no_tools {
        "disabled".to_string()
    } else {
        "enabled (Read, Write, Edit, Bash, Glob, Grep)".to_string()
    };

    let session_line = match &app.session {
        Some(s) => format!("{} ({} turns, {} tokens)",
            s.name,
            s.turn_count(),
            s.total_usage.input_tokens.saturating_add(s.total_usage.output_tokens),
        ),
        None => "(ephemeral — no session)".to_string(),
    };

    let sessions_dir = app
        .args
        .sessions_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| match caliban_sessions::SessionStore::default_root() {
            Ok(p) => p.display().to_string(),
            Err(_) => "(unavailable)".to_string(),
        });

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
    out.push(Line::styled("  Press q or Esc to close.", Style::default().add_modifier(Modifier::DIM)));
    out
}
```

- [ ] **Step 2: Wire `/config` slash command**

In `handle_slash_command`, add a `/config` arm:

```rust
"/config" => {
    app.view = ViewState::Overlay(Overlay::Config);
}
```

Also update the `/help` overlay's command list to include `/config` (already shown in U.2's `slash_help_lines`).

- [ ] **Step 3: Build + manual test + commit**

```bash
cargo build  --bin caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

Manual: `./target/debug/caliban --session foo` → `/config` → see provider/model/etc. populated. `q` → main view.

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(tui): /config overlay (active configuration)

New /config slash command opens an overlay showing live values for
provider, model, max_tokens, max_turns, temperature, workspace root,
restrict_paths, tool enablement, sessions dir, active session, and
quiet mode. Read-only in v1; editing is a future enhancement.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task U.4: `/mcp` + `/skills` stub overlays

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Implement `mcp_lines()`**

```rust
fn mcp_lines() -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![Line::raw("")];
    out.push(Line::raw("   No MCP servers configured."));
    out.push(Line::raw(""));
    out.push(Line::raw("   MCP (Model Context Protocol) lets caliban consume external"));
    out.push(Line::raw("   tool servers — for example, a SilverBullet notebook, a"));
    out.push(Line::raw("   Linear ticket browser, or a custom in-house server."));
    out.push(Line::raw(""));
    out.push(Line::styled("   Planned configuration:", Style::default().fg(Color::Yellow)));
    out.push(Line::raw("     ~/.config/caliban/mcp.toml"));
    out.push(Line::raw(""));
    out.push(Line::styled("   Example (future):", Style::default().fg(Color::Yellow)));
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
    out.push(Line::styled("   See caliban-mcp-client (Layer 2 sub-project) — not yet shipped.", dim));
    out.push(Line::raw(""));
    out.push(Line::styled("  Press q or Esc to close.", dim));
    out
}
```

- [ ] **Step 2: Implement `skills_lines()`**

```rust
fn skills_lines() -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![Line::raw("")];
    out.push(Line::raw("   No skills configured."));
    out.push(Line::raw(""));
    out.push(Line::raw("   Skills are reusable instruction-and-procedure packages the"));
    out.push(Line::raw("   model can invoke via a Skill tool. They mirror Claude"));
    out.push(Line::raw("   Code's superpowers / skills design."));
    out.push(Line::raw(""));
    out.push(Line::styled("   Planned configuration:", Style::default().fg(Color::Yellow)));
    out.push(Line::raw("     ~/.config/caliban/skills/"));
    out.push(Line::raw("         <skill-name>/"));
    out.push(Line::raw("             SKILL.md         (instruction set)"));
    out.push(Line::raw("             scripts/         (optional helper scripts)"));
    out.push(Line::raw("             references/      (optional reference docs)"));
    out.push(Line::raw(""));
    out.push(Line::raw("   The Skill tool would dispatch to the matching skill, load"));
    out.push(Line::raw("   its SKILL.md, and inject the content into the agent's"));
    out.push(Line::raw("   context."));
    out.push(Line::raw(""));
    out.push(Line::styled("   See caliban-skills (future sub-project) — not yet shipped.", dim));
    out.push(Line::raw(""));
    out.push(Line::styled("  Press q or Esc to close.", dim));
    out
}
```

- [ ] **Step 3: Wire `/mcp` and `/skills` slash commands**

In `handle_slash_command`:

```rust
"/mcp" => {
    app.view = ViewState::Overlay(Overlay::Mcp);
}
"/skills" => {
    app.view = ViewState::Overlay(Overlay::Skills);
}
```

- [ ] **Step 4: Build + manual test + commit**

```bash
cargo build  --bin caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

Manual: open caliban; type `/mcp` → see stub overlay. `q`. `/skills` → see stub overlay. `q`.

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(tui): /mcp and /skills stub overlays

Two new overlays documenting the planned shape of MCP server
configuration and skills/SKILL.md loaders. Read-only stubs that point
at the future Layer-2 sub-projects (caliban-mcp-client and
caliban-skills) and their planned config locations
(~/.config/caliban/mcp.toml, ~/.config/caliban/skills/).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task U.5: ADR 0013 + README

**Files:**
- Create: `adrs/0013-tui-overlays.md`
- Modify: `adrs/README.md`
- Modify: `README.md`

- [ ] **Step 1: ADR 0013**

```markdown
# ADR 0013 · TUI overlays + layout v2 (input bracketed by horizontal rules)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

The first TUI iteration shipped a working three-region layout (output |
status | input) and slash commands that wrote to the transcript. As the
slash-command list grew (help, config, mcp, skills) the transcript
became a cluttered place to render reference information.

## Decision

1. **Layout v2** reorders the regions so the input area sits between
   the output region and the status bar, bracketed by single-row
   horizontal rules. This puts the active input visually closer to the
   bottom (where the user's hands rest) and matches the Claude Code
   layout the user requested.

2. **Overlays** are modal popups rendered centered (80% × 80%) over
   the main view via ratatui's `Clear` + bordered `Block` + `Paragraph`
   widgets. `ViewState::Overlay(Overlay)` on `App` tracks which
   overlay is active; `Esc` or `q` resets to `ViewState::Main`. Main-
   view key handling is suppressed while an overlay is open (the
   overlay is read-only in v1).

3. **Four sub-menus:** `/help` (slash command + key reference),
   `/config` (active configuration from `app.args`/`app.session`),
   `/mcp` (stub pointing at future caliban-mcp-client), `/skills`
   (stub pointing at future caliban-skills).

## Consequences

- **Positive:** Reference views don't pollute the transcript. The
  layout is closer to Claude Code's. The /config view is genuinely
  useful for verifying caliban's state at a glance. The /mcp and
  /skills stubs document the future direction in the UI itself.
- **Negative:** Two more enum variants per addition; overlay content
  is static for now and must be hand-edited when slash commands
  evolve. Editing config from the UI is deferred.
- **Revisit if:** A keyboard-driven command palette (Ctrl+P-style) is
  desired; if the slash-command list grows beyond ~12 entries and
  needs categorization; if /config gains edit capability (toggling
  bools, changing model mid-session) requiring stateful focus tracking.
```

- [ ] **Step 2: Append to `adrs/README.md`**

```
| [0013](0013-tui-overlays.md) | TUI overlays + layout v2 | accepted |
```

- [ ] **Step 3: Update root `README.md`**

In the existing "Interactive TUI" section, update the ASCII mock to show the new layout (status bar at the bottom). Add a short paragraph after the mock:

```
Type `/help`, `/config`, `/mcp`, or `/skills` to open a sub-menu overlay
showing slash command reference, active configuration, planned MCP
server config, or planned skills config respectively. `Esc` or `q`
closes any overlay.
```

Update the slash-command list to include `/config`, `/mcp`, `/skills`.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all -- --check
cargo build  --workspace
cargo test   --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add adrs/ README.md
git commit -m "$(cat <<'EOF'
docs: ADR 0013 + README for TUI overlays + layout v2

ADR 0013 captures the rationale for moving the status bar to the
bottom, surrounding the input with horizontal rules, and the new
overlay infrastructure backing /help, /config, /mcp, /skills.

README updates the ASCII mock-up of the TUI layout and documents the
four sub-menu overlays.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

Spec coverage: layout (U.1), overlay infrastructure + /help (U.2), /config (U.3), /mcp + /skills (U.4), docs (U.5). All five overlay variants from the spec are implemented. Type consistency: `ViewState`/`Overlay` defined in U.2 used by U.3 + U.4. Slash command handler centrally dispatches to `app.view = ViewState::Overlay(...)`.

Risks: overlay-while-streaming works because each frame re-renders both layers. Static content in /mcp /skills is intentional — they're stubs, not live data.
