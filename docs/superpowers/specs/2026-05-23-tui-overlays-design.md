# TUI Overlays + Layout v2 · Design

- **Date:** 2026-05-23
- **Status:** Draft
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** TUI sub-project
- **Next sub-project:** TBD — memory, MCP wiring, or model-router

## Goals

Three changes to the TUI:

1. **Layout flip** — move the input area above the status bar, and bracket the input with horizontal-line borders top and bottom. Final order: output → border → input → border → status bar.

2. **Overlay infrastructure** — modal popups rendered over the main view, activated via slash commands. Dismissed with `Esc` or `q`. The main view stays underneath but is dimmed/blocked behind the overlay.

3. **Four sub-menu overlays** —
   - `/help` (already a transcript line) becomes a proper overlay showing all slash commands with descriptions and keyboard shortcuts.
   - `/config` shows active configuration (provider, model, max_tokens, max_turns, temperature, workspace root, restrict_paths, no_tools, sessions_dir).
   - `/mcp` is a stub view: "No MCP servers configured. Coming soon — configure via ~/.config/caliban/mcp.toml" + an empty placeholder list.
   - `/skills` is a stub view: same pattern, anchored on a future `~/.config/caliban/skills/` directory.

**Acceptance:** Running `caliban` shows the new layout (input bracketed by borders, status bar at the bottom). Typing `/help` opens a popup overlay listing all slash commands. `/config`, `/mcp`, `/skills` each open their own overlay. `Esc` or `q` closes any overlay and returns to the main view.

## Non-goals

- Editing config from the `/config` view — read-only for v1. Future work: keyboard shortcuts to toggle bools, edit text fields.
- Functional MCP / Skills support — these are stubs documenting the planned shape. Real implementation lives in future Layer 2 sub-projects (MCP client, skills loader).
- Tabs / multi-pane within an overlay — overlays are single-paragraph views in v1.
- Markdown rendering in overlays — plain text + minimal styling only.
- Command palette / fuzzy search — slash commands remain the access mechanism.

## Layout v2

```
┌────────────────────────────────────────────────────────────────┐
│ user: What's in README.md?                                     │
│                                                                │
│ 🔧 Read({"path":"README.md"})                                  │
│    → → Read README.md, lines 1-83 of 83                        │
│                                                                │
│ assistant: It's a Rust agent harness called caliban…           │
│                                                                │
│ [caliban: 2 turns · 132↑ 48↓ tokens]                           │
├────────────────────────────────────────────────────────────────┤
│ > █                                                            │
├────────────────────────────────────────────────────────────────┤
│ ~/dev/personal/iron-orrery · openai gpt-4o · session: research │
└────────────────────────────────────────────────────────────────┘
```

Three-region layout becomes five rows when you count the borders:

```rust
Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(0),      // output region (flex)
        Constraint::Length(1),   // top border above input
        Constraint::Length(1),   // input area
        Constraint::Length(1),   // bottom border below input
        Constraint::Length(1),   // status bar
    ])
    .split(area)
```

The top + bottom borders are 1-row-tall `Block` widgets with only the top/bottom border edge drawn (no left/right). Equivalent to a horizontal rule.

```rust
let hrule = Block::default().borders(Borders::TOP);
frame.render_widget(hrule, top_border_chunk);
```

(Or just a `Line` filled with `─` characters spanning the area width — simpler and avoids ratatui's border-corner-joining quirks.)

## Overlay infrastructure

### `ViewState`

```rust
pub enum ViewState {
    Main,
    Overlay(Overlay),
}

pub enum Overlay {
    SlashHelp,
    Config,
    Mcp,
    Skills,
}
```

`App::view: ViewState` (defaults to `Main`).

### Rendering

When `ViewState::Overlay(o)`:

1. Render the main view first (output + input + status) — even though it'll be covered, this preserves the layout and any partially-streamed text.
2. Compute a centered rect (80% width × 70% height).
3. Use `frame.render_widget(Clear, overlay_area)` to clear that area.
4. Render a bordered `Block` with the overlay's title.
5. Render the overlay's content inside that block.

```rust
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect { ... }
```

### Input handling

When an overlay is open:
- `Esc` or `q` → `app.view = ViewState::Main`.
- Other keys → no-op (overlays are read-only in v1).
- `Ctrl+C` → if a turn is running, cancel it (overlay stays open). Otherwise close overlay.
- `Ctrl+D` → no-op (must close overlay first to exit).

When on main view:
- All existing key handling continues.
- Slash commands `/help`, `/config`, `/mcp`, `/skills` set `app.view = ViewState::Overlay(...)`.

### Status bar update

The status bar shows the current view's name when in an overlay:
- Main view: existing format
- Overlay: `[help]`, `[config]`, `[mcp]`, `[skills]` prefix or suffix indicating you're in a sub-menu and `q`/`Esc` returns to main.

Example: `~/dev/personal/iron-orrery · openai gpt-4o · session: research · [config — q to close]`

## Overlay contents

### `/help` — Slash command reference

```
┌─ Slash Commands ───────────────────────────────────────────────┐
│                                                                │
│   /help              Show this help                            │
│   /exit, /quit       Save session and exit                     │
│   /clear             Clear transcript                          │
│   /sessions          List saved sessions                       │
│   /save [<name>]     Save current session (optionally rename)  │
│   /usage             Show accumulated usage                    │
│   /config            Show active configuration                 │
│   /mcp               MCP server configuration (stub)           │
│   /skills            Skills configuration (stub)               │
│                                                                │
│ ── Keyboard ───────────────────────────────────────────────────│
│   Enter              Submit prompt or slash command            │
│   Backspace / Del    Edit input                                │
│   Left / Right       Move cursor                               │
│   Up / Down          Navigate input history                    │
│   Home / End         Jump to start / end of input              │
│   PageUp / PageDn    Scroll transcript                         │
│   Ctrl+C             Cancel running turn or clear input        │
│   Ctrl+D             Exit (when input is empty)                │
│   Ctrl+L             Clear transcript                          │
│   Esc / q            Close this overlay                        │
│                                                                │
│  Press q or Esc to close.                                      │
└────────────────────────────────────────────────────────────────┘
```

Content is a static `Paragraph` with fixed lines. Header styled bold; commands left-padded with 3 spaces.

### `/config` — Active configuration

```
┌─ Configuration ────────────────────────────────────────────────┐
│                                                                │
│   Provider          openai                                     │
│   Model             gpt-4o                                     │
│   Max tokens        2048                                       │
│   Max turns         50                                         │
│   Temperature       (default)                                  │
│                                                                │
│   Workspace root    ~/dev/personal/iron-orrery                 │
│   Restrict paths    false                                      │
│   Tools             enabled (Read, Write, Edit, Bash, Glob,    │
│                              Grep)                             │
│                                                                │
│   Sessions dir      ~/.local/share/caliban/sessions            │
│   Active session    research (5 turns, 4682 tokens)            │
│                                                                │
│   Quiet mode        false                                      │
│                                                                │
│  Press q or Esc to close.                                      │
└────────────────────────────────────────────────────────────────┘
```

Pulled from `app.args` and `app.session`. Tools list reflects what's actually registered (currently always all six unless `--no-tools`).

### `/mcp` — MCP servers (stub)

```
┌─ MCP Servers ──────────────────────────────────────────────────┐
│                                                                │
│   No MCP servers configured.                                   │
│                                                                │
│   MCP (Model Context Protocol) lets caliban consume external   │
│   tool servers — for example, a SilverBullet notebook, a       │
│   Linear ticket browser, or a custom in-house server.          │
│                                                                │
│   Planned configuration:                                       │
│     ~/.config/caliban/mcp.toml                                  │
│                                                                │
│   Example (future):                                            │
│     [[server]]                                                 │
│     name = "silverbullet"                                      │
│     transport = "stdio"                                        │
│     command = "sb-mcp"                                         │
│     args = ["--vault", "~/notes"]                              │
│                                                                │
│     [[server]]                                                 │
│     name = "linear"                                            │
│     transport = "http"                                         │
│     url = "https://mcp.example.com/linear"                     │
│                                                                │
│   See caliban-mcp-client (Layer 2 sub-project) — not yet       │
│   shipped.                                                     │
│                                                                │
│  Press q or Esc to close.                                      │
└────────────────────────────────────────────────────────────────┘
```

Static text. No live data yet.

### `/skills` — Skills (stub)

```
┌─ Skills ───────────────────────────────────────────────────────┐
│                                                                │
│   No skills configured.                                        │
│                                                                │
│   Skills are reusable instruction-and-procedure packages the   │
│   model can invoke via a Skill tool. They mirror Claude        │
│   Code's superpowers / skills design.                          │
│                                                                │
│   Planned configuration:                                       │
│     ~/.config/caliban/skills/                                  │
│         <skill-name>/                                          │
│             SKILL.md         (instruction set)                 │
│             scripts/         (optional helper scripts)         │
│             references/      (optional reference docs)         │
│                                                                │
│   The Skill tool would dispatch to the matching skill, load    │
│   its SKILL.md, and inject the content into the agent's        │
│   context.                                                     │
│                                                                │
│   See caliban-skills (future sub-project) — not yet shipped.   │
│                                                                │
│  Press q or Esc to close.                                      │
└────────────────────────────────────────────────────────────────┘
```

Static text.

## Implementation strategy

Five tasks:

- **U.1 — Layout v2 + input borders.** Reorder regions, add the two horizontal rules. Small.
- **U.2 — Overlay infrastructure + `/help` overlay.** Add `ViewState` + centered-rect helper + render path for overlays. Convert the existing `/help` transcript line to an overlay. Medium.
- **U.3 — `/config` overlay.** New slash command + overlay content building from app state. Small.
- **U.4 — `/mcp` + `/skills` stub overlays.** Two new slash commands + their static content. Small.
- **U.5 — Status bar tweak + ADR 0013 + README.** Status bar shows overlay indicator. Docs. Small.

## Acceptance criteria

- `cargo build --bin caliban` clean.
- `cargo test --workspace` continues to pass (no behavioral regressions for non-overlay code paths).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- TUI layout has input above status bar with horizontal lines around the input.
- `/help`, `/config`, `/mcp`, `/skills` each open a centered overlay with the documented content.
- `Esc` or `q` closes any open overlay.
- Status bar shows current overlay indicator (e.g., `[config — q to close]`) when one is open.
- One ADR (0013) covers the overlay design choices.
- README updated to mention the sub-menus.

## Risks

- **Overlay-while-streaming.** If the user opens `/config` while a turn is streaming, text deltas still arrive. The main view's transcript continues to grow underneath the overlay. The overlay re-renders each frame; no issue. The main view will reflect accumulated text when the overlay closes.
- **Stale overlay content.** `/config` reads from `app.args` and `app.session` snapshot-style on each render — fresh data on every redraw, no caching.
- **Terminal width truncation.** If the terminal is narrower than the overlay's intended width, ratatui will clip. The centered-rect helper sizes proportionally so smaller terminals get smaller overlays. Content uses `wrap(true)` so long lines reflow.
- **Color contrast in overlays.** Use only dim/bold modifiers; avoid colored backgrounds that might be unreadable in light terminals.
- **Esc key conflicts.** Currently Esc cancels a running turn. Once an overlay is open, Esc closes the overlay. If a turn is running AND an overlay is open, Esc closes the overlay (and the turn keeps running) — second Esc cancels the turn. Documented.
