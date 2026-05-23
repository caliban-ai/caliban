# REPL + Sessions · Design

- **Date:** 2026-05-23
- **Status:** Draft
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** Layer 1 (B+C+D) + Layer 4 CLI
- **Next sub-project:** TBD — MCP client, memory, or model-router are all natural candidates

## Goals

Two related features that together transform caliban from single-shot to a real working tool:

1. **Session persistence** — `caliban --session <name>` loads conversation history from disk before the run and saves it after. New sessions are auto-created. Conversations resume across invocations.
2. **Interactive REPL** — `caliban` with no prompt enters a read-eval-print loop. User types prompts; agent runs to completion; result is rendered; prompt again. Slash commands (`/help`, `/exit`, `/clear`, `/sessions`, `/load`, `/save`, `/usage`, `/model`) provide session and config control without exiting.

**Acceptance:**

```bash
caliban --session research
# (interactive REPL begins; on first run, "research" session is created)
> What's in README.md?
[model uses Read tool, summarizes]
> Now look at Cargo.toml — what's the latest crate?
[model uses Read tool on Cargo.toml, references both]
> /exit
$ caliban --session research "What did I ask first?"
[answers about README.md — history was preserved]
```

## Non-goals

- TUI (cursor positioning, multi-pane layout) — separate sub-project.
- Concurrent multi-session orchestration — single-session at a time.
- Session sharing / cloud sync — local-disk JSON only.
- Tool-approval interactivity (`--ask`) — still deferred.
- Editing history mid-session (deleting a previous message) — not in v1; could be a `/edit` slash command later.
- Branching/forking a session — not in v1; user can copy a JSON file by hand.

## New crate: `caliban-sessions`

A small library crate housing session persistence. Lives at `crates/caliban-sessions/`. Used by `caliban/` (and later by `caliban-tui/`, `caliban-orchestrator/`).

### Public API

```rust
pub struct PersistedSession {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub messages: Vec<Message>,
    pub total_usage: Usage,
}

impl PersistedSession {
    pub fn new(name: impl Into<String>, provider: impl Into<String>, model: impl Into<String>) -> Self;
    pub fn touch(&mut self);  // update updated_at to now
    pub fn merge_run(&mut self, new_messages: &[Message], added_usage: Usage);
}

pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self;
    pub fn default_root() -> Result<PathBuf, Error>;   // resolves ~/.local/share/caliban/sessions/ (XDG_DATA_HOME aware)
    pub fn load(&self, name: &str) -> Result<Option<PersistedSession>, Error>;
    pub fn save(&self, session: &PersistedSession) -> Result<(), Error>;
    pub fn list(&self) -> Result<Vec<SessionMetadata>, Error>;
    pub fn delete(&self, name: &str) -> Result<(), Error>;
    pub fn path_for(&self, name: &str) -> PathBuf;
}

pub struct SessionMetadata {
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub turn_count: u32,
    pub total_tokens: u32,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid session name '{0}': must match [a-zA-Z0-9_-]+")]
    InvalidName(String),
    #[error("home directory not found")]
    NoHome,
}
```

**Storage layout:**

```
~/.local/share/caliban/sessions/
├── research.json
├── coding.json
└── ...
```

Each `.json` file contains the full `PersistedSession` serialized via serde_json (pretty-printed for human readability — useful when debugging). Atomic write (`tempfile::NamedTempFile::persist`) so a crash mid-save can't corrupt the file.

**Name validation:** session names must match `^[a-zA-Z0-9_-]+$` to avoid path-traversal and platform-incompatible characters. Invalid names → `Error::InvalidName`.

**XDG handling:** `default_root()` resolves to:
1. `$XDG_DATA_HOME/caliban/sessions` if set
2. else `$HOME/.local/share/caliban/sessions`

Directory is created on first save with `fs::create_dir_all`.

## CLI changes

### New flags

```
--session <NAME>           Load or create a named session. Use the literal string for the session name.
                           Sessions persist to ~/.local/share/caliban/sessions/<NAME>.json.
--no-save                  Don't write the session back to disk after the run.
                           Useful for one-off experiments without polluting an existing session.
--sessions-dir <DIR>       Override the sessions directory (default: ~/.local/share/caliban/sessions).
```

### Three modes of invocation

1. **Single-prompt, ephemeral** (current behavior): `caliban "prompt"` — no session involved.
2. **Single-prompt, persistent**: `caliban --session <name> "prompt"` — load session if exists, run agent, save back. The agent sees prior turns as initial history.
3. **REPL**: `caliban` (no prompt). Enters interactive mode. If `--session <name>` given, REPL is tied to that session. Otherwise an unnamed in-memory-only session that lasts until exit.

REPL is only entered when:
- No prompt positional / `--prompt` is supplied AND
- stdin is a TTY (so `echo "x" | caliban` still works as expected — that path treats stdin as the prompt because `"-"` was the convention; we keep that but `caliban` with no args + no piped stdin → REPL).

Detection: `is_terminal::IsTerminal` (the standard crate; in std as of Rust 1.70).

### REPL UI

```
caliban v0.0.0 — openai gpt-4o — session: research (3 turns, 4.5k tokens)
Type your message; /help for commands; /exit or Ctrl-D to quit.

> What's in README.md?
[...stream of model output and tool calls...]
[caliban: 2 turns · 132↑ 48↓ tokens]

> /usage
session research: 5 total turns, 4682 input + 1294 output tokens

> /model claude-3-5-sonnet
[switches provider to anthropic, model to claude-3-5-sonnet for the next turn]

> /exit
saved session 'research' (5 turns, 5976 tokens).
```

### Slash commands

| Command | Effect |
|---|---|
| `/help` | Print available commands. |
| `/exit` or `/quit` | Save session (if persistent) and exit. |
| `/clear` | Clear in-memory message history. For persistent sessions, the next save overwrites with the cleared state — confirm with y/N. |
| `/sessions` | List sessions in the sessions directory with their metadata. |
| `/load <name>` | Save current session, then load `<name>` (or create if missing). |
| `/save [<name>]` | Save the current session. With a name, save AS that name (effectively copies). |
| `/usage` | Print accumulated session usage stats. |
| `/model <name>` | Change the model for subsequent turns. Doesn't change the provider. |
| `/provider <name>` | Change provider AND default model for subsequent turns. Will fail if the relevant env var isn't set. |
| `/tools` | List currently registered tools. |
| `/no-tools` / `/tools-on` | Toggle tool dispatch. |

Slash commands are detected by leading `/`. Anything else is sent to the agent as a user message.

### Cancellation in REPL

`Ctrl+C` during a running turn cancels that turn (existing logic). The REPL catches the resulting `Error::Cancelled` and returns to the prompt without exiting. A second `Ctrl+C` within 500ms or `Ctrl+C` at the prompt (no turn running) exits gracefully.

### Stdin handling

- `caliban -` reads prompt from stdin (existing behavior).
- `caliban < file.txt` (no `-`) and `caliban` with no args + piped stdin: also read stdin as prompt for backwards compatibility.
- `caliban` with no args + TTY stdin: REPL.

### Output streaming differences

The single-prompt mode currently writes the assistant text to stdout. In REPL mode, we want the same but with a clearer turn-end marker and a `> ` prompt afterward. Implementation: write the assistant text + a final newline + the usage summary; then print `> ` and wait for the next line.

Tool announcements still go to stderr in REPL mode — the user sees them interleaved with their typing if a tool call is mid-flight, but that's fine because the REPL prompt only returns AFTER the turn finishes.

## Crate dependencies

`caliban-sessions/Cargo.toml`:

```toml
[dependencies]
caliban-provider = { path = "../caliban-provider" }
serde            = { workspace = true }
serde_json       = { workspace = true }
thiserror        = { workspace = true }
chrono           = { version = "0.4", features = ["serde"] }
tempfile         = { workspace = true }  # for atomic writes
dirs             = "5"  # for XDG/home resolution

[dev-dependencies]
tempfile = { workspace = true }
```

Add `chrono` and `dirs` to root `[workspace.dependencies]`.

`caliban/Cargo.toml` gains:

```toml
caliban-sessions = { path = "../crates/caliban-sessions" }
is-terminal      = "0.4"  # or use std::io::IsTerminal directly (stable since 1.70)
rustyline        = { version = "14", features = ["with-file-history"] }
```

(`rustyline` provides a proper line editor with arrow-key history. Without it, the REPL would have a bare `stdin().lines()` UX that's unpleasant.)

## Implementation strategy

The work splits into four tasks:

1. **caliban-sessions skeleton + PersistedSession + SessionStore + tests.** Self-contained; can be reviewed and merged independently.
2. **CLI: --session flag, single-prompt persistent mode.** Wire load → existing single-prompt flow → save.
3. **CLI: REPL mode.** Add the rustyline-driven loop + slash-command parser + plumbing into the existing event-rendering code.
4. **ADR + README update.**

## Acceptance criteria

- `crates/caliban-sessions/` exists; workspace member added.
- `cargo test --workspace` passes — at least 8 new tests in `caliban-sessions` (PersistedSession round-trip, SessionStore save/load/list/delete, atomic-write under simulated mid-write crash via TempDir, name validation rejects `../traversal`).
- Plus at least one new integration test in `caliban/tests/` covering the persistent-session flow (write a session file, invoke `--session <name>` via a small driver, verify the file was updated).
- `caliban` (no args, TTY) enters REPL. Verified manually (REPL is hard to automate end-to-end).
- `caliban --session foo "prompt"` works as single-shot-with-history.
- `caliban --session foo` (no prompt, TTY) enters REPL anchored to that session.
- Slash commands `/exit`, `/help`, `/clear`, `/sessions`, `/load`, `/save`, `/usage` all work.
- ADR 0011 captures the persistence design.
- README has a "Sessions" section with examples.

## Risks

- **`rustyline` adds a noticeable dep tree.** It pulls `unicode-*` crates and a tty backend. Acceptable for an interactive CLI. If users want a minimal binary, they can use single-shot mode without REPL.
- **Atomic writes on Windows.** `tempfile::NamedTempFile::persist` is cross-platform but Windows requires the target file to not be open elsewhere. We don't keep file handles open, so this should work. Not tested on Windows initially.
- **JSON pretty-print bloat.** Pretty-printing makes files larger but is reviewable by hand. A future flag could switch to compact format.
- **Slash-command future-proofing.** Adding new commands shouldn't break existing usage. The handler returns `enum CommandOutcome { Continue, Exit, RunPrompt(String) }` so adding a new variant is internal.
- **Race if user runs two caliban processes against the same session.** Last-write-wins; could corrupt history. Documented; out of scope for a single-user MVP.
