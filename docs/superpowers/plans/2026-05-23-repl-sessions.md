# REPL + Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Add session persistence (`--session <name>`) + interactive REPL mode (`caliban` with no prompt) so caliban becomes daily-usable.

**Architecture:** New `caliban-sessions` crate (PersistedSession + SessionStore + JSON-on-disk persistence). CLI gains three modes: single-prompt ephemeral (current), single-prompt persistent (new), REPL (new). REPL uses `rustyline` for line editing + history. Slash commands route to handlers that return `CommandOutcome::{Continue, Exit, RunPrompt}`.

**Tech Stack:** Rust 1.85.0, serde + serde_json (persistence), chrono (timestamps), dirs (XDG), tempfile (atomic write), rustyline (line editor), std::io::IsTerminal (TTY detection).

**Spec:** [`docs/superpowers/specs/2026-05-23-repl-sessions-design.md`](../specs/2026-05-23-repl-sessions-design.md)

---

## Task 1: `caliban-sessions` crate (PersistedSession + SessionStore)

**Files:**
- Modify: root `Cargo.toml` (workspace member + `chrono` and `dirs` workspace deps)
- Create: `crates/caliban-sessions/Cargo.toml`
- Create: `crates/caliban-sessions/src/lib.rs`
- Create: `crates/caliban-sessions/src/error.rs`
- Create: `crates/caliban-sessions/src/session.rs`
- Create: `crates/caliban-sessions/src/store.rs`
- Create: `crates/caliban-sessions/tests/store.rs`

- [ ] **Step 1: Root Cargo.toml**

Add `"crates/caliban-sessions"` to workspace members.

Add to `[workspace.dependencies]`:
```toml
chrono = { version = "0.4", default-features = false, features = ["serde", "clock"] }
dirs   = "5"
```

- [ ] **Step 2: `crates/caliban-sessions/Cargo.toml`**

```toml
[package]
name        = "caliban-sessions"
version     = "0.0.0"
description = "Session persistence for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[dependencies]
caliban-provider = { path = "../caliban-provider" }
serde            = { workspace = true }
serde_json       = { workspace = true }
thiserror        = { workspace = true }
chrono           = { workspace = true }
tempfile         = { workspace = true }
dirs             = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: `src/error.rs`**

```rust
//! Errors for session persistence.

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid session name '{0}': must match [a-zA-Z0-9_-]+ and be 1..=64 chars")]
    InvalidName(String),
    #[error("home directory not found")]
    NoHome,
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 4: `src/session.rs`**

```rust
//! PersistedSession — a saveable conversation.

use caliban_provider::{Message, Role, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A conversation session, suitable for persisting to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Construct a new empty session.
    #[must_use]
    pub fn new(name: impl Into<String>, provider: impl Into<String>, model: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            created_at: now,
            updated_at: now,
            provider: provider.into(),
            model: model.into(),
            messages: Vec::new(),
            total_usage: Usage::default(),
        }
    }

    /// Update `updated_at` to now.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    /// Replace messages with `new_messages` (typically the `RunOutcome.final_messages`)
    /// and add the run's `Usage` to the cumulative total.
    pub fn merge_run(&mut self, new_messages: Vec<Message>, added_usage: Usage) {
        self.messages = new_messages;
        self.total_usage.merge(added_usage);
        self.touch();
    }

    /// Count how many turn-pairs (User → Assistant) are in this session's history.
    #[must_use]
    pub fn turn_count(&self) -> u32 {
        u32::try_from(self.messages.iter().filter(|m| m.role == Role::Assistant).count()).unwrap_or(u32::MAX)
    }
}
```

- [ ] **Step 5: `src/store.rs`**

```rust
//! SessionStore — disk-backed CRUD over PersistedSession.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::error::{Error, Result};
use crate::session::PersistedSession;

const MAX_NAME_LEN: usize = 64;

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(Error::InvalidName(name.into()));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(Error::InvalidName(name.into()));
    }
    Ok(())
}

/// On-disk session store.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// Construct a store with the given root directory.
    #[must_use]
    pub fn new(root: PathBuf) -> Self { Self { root } }

    /// Resolve the default root: `$XDG_DATA_HOME/caliban/sessions`
    /// or `$HOME/.local/share/caliban/sessions`.
    ///
    /// # Errors
    /// Returns `Error::NoHome` if neither XDG_DATA_HOME nor HOME are available.
    pub fn default_root() -> Result<PathBuf> {
        let base = dirs::data_dir().ok_or(Error::NoHome)?;
        Ok(base.join("caliban").join("sessions"))
    }

    /// Get the path for a named session.
    #[must_use]
    pub fn path_for(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.json"))
    }

    /// Load a session by name. Returns Ok(None) if the file doesn't exist.
    ///
    /// # Errors
    /// I/O, deserialization, or name-validation errors.
    pub fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        validate_name(name)?;
        let path = self.path_for(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Save a session atomically.
    ///
    /// # Errors
    /// I/O, serialization, or name-validation errors.
    pub fn save(&self, session: &PersistedSession) -> Result<()> {
        validate_name(&session.name)?;
        std::fs::create_dir_all(&self.root)?;
        let serialized = serde_json::to_vec_pretty(session)?;
        // Atomic write: write to temp file in the same dir, then persist (rename).
        let tmp = NamedTempFile::new_in(&self.root)?;
        std::fs::write(tmp.path(), &serialized)?;
        let target = self.path_for(&session.name);
        tmp.persist(&target)
            .map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    /// List sessions (their metadata) sorted by `updated_at` descending.
    ///
    /// # Errors
    /// I/O errors. Individual broken files are SKIPPED with no error.
    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(session): std::result::Result<PersistedSession, _> = serde_json::from_slice(&bytes) else { continue };
            out.push(SessionMetadata {
                name: session.name,
                updated_at: session.updated_at,
                turn_count: u32::try_from(session.messages.iter().filter(|m| m.role == caliban_provider::Role::Assistant).count()).unwrap_or(u32::MAX),
                total_tokens: session.total_usage.input_tokens.saturating_add(session.total_usage.output_tokens),
            });
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    /// Delete a session.
    ///
    /// # Errors
    /// I/O or name-validation errors.
    pub fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let path = self.path_for(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Metadata returned by `SessionStore::list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub turn_count: u32,
    pub total_tokens: u32,
}
```

- [ ] **Step 6: `src/lib.rs`**

```rust
//! Session persistence for the caliban agent harness.
//!
//! Stores conversation history as JSON files under
//! `$XDG_DATA_HOME/caliban/sessions/` (default
//! `$HOME/.local/share/caliban/sessions/`).

pub mod error;
pub mod session;
pub mod store;

pub use error::{Error, Result};
pub use session::PersistedSession;
pub use store::{SessionMetadata, SessionStore};
```

- [ ] **Step 7: `tests/store.rs`**

```rust
#![allow(missing_docs)]

use caliban_provider::{Message, Usage};
use caliban_sessions::{PersistedSession, SessionStore};
use tempfile::TempDir;

fn fake_session() -> PersistedSession {
    let mut s = PersistedSession::new("test", "anthropic", "claude-3-5-sonnet");
    s.messages.push(Message::user_text("hi"));
    s.messages.push(Message::assistant_text("hi back"));
    s.total_usage = Usage { input_tokens: 5, output_tokens: 3, cache_creation_input_tokens: None, cache_read_input_tokens: None };
    s
}

#[test]
fn save_and_load_round_trip() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let original = fake_session();
    store.save(&original).unwrap();
    let loaded = store.load("test").unwrap().unwrap();
    assert_eq!(loaded.name, original.name);
    assert_eq!(loaded.provider, original.provider);
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.total_usage.input_tokens, 5);
}

#[test]
fn load_missing_returns_none() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let loaded = store.load("nonexistent").unwrap();
    assert!(loaded.is_none());
}

#[test]
fn list_returns_sorted_by_updated_at_desc() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let mut a = PersistedSession::new("a", "anthropic", "m");
    a.updated_at = chrono::DateTime::from_timestamp(1000, 0).unwrap();
    let mut b = PersistedSession::new("b", "anthropic", "m");
    b.updated_at = chrono::DateTime::from_timestamp(2000, 0).unwrap();
    store.save(&a).unwrap();
    store.save(&b).unwrap();
    let list = store.list().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "b"); // b is newer; comes first
    assert_eq!(list[1].name, "a");
}

#[test]
fn delete_removes_file() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.save(&fake_session()).unwrap();
    assert!(store.load("test").unwrap().is_some());
    store.delete("test").unwrap();
    assert!(store.load("test").unwrap().is_none());
}

#[test]
fn delete_missing_succeeds() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.delete("nonexistent").unwrap();
}

#[test]
fn invalid_name_rejected() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    assert!(store.load("../escape").is_err());
    assert!(store.load("name with spaces").is_err());
    assert!(store.load("").is_err());
}

#[test]
fn invalid_name_in_save_rejected() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let mut bad = PersistedSession::new("bad name", "anthropic", "m");
    bad.messages.push(Message::user_text("x"));
    assert!(store.save(&bad).is_err());
}

#[test]
fn pretty_json_is_human_readable() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.save(&fake_session()).unwrap();
    let path = store.path_for("test");
    let bytes = std::fs::read(&path).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\n"), "pretty-printed JSON should have newlines");
    assert!(text.contains("\"name\""));
}
```

- [ ] **Step 8: Build + test + commit**

```bash
cargo build  -p caliban-sessions
cargo test   -p caliban-sessions
cargo clippy -p caliban-sessions --all-targets -- -D warnings
cargo fmt --all -- --check
git add Cargo.toml crates/caliban-sessions/
git commit -m "$(cat <<'EOF'
feat(sessions): caliban-sessions crate

PersistedSession (name, provider, model, messages, total_usage,
created_at/updated_at) + SessionStore (atomic JSON-on-disk save,
load, list-sorted-by-updated-at, delete). Names validated against
[a-zA-Z0-9_-]+ to prevent path traversal. Default root resolves to
$XDG_DATA_HOME/caliban/sessions (or $HOME/.local/share/...).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: CLI `--session` flag (persistent single-prompt mode)

**Files:**
- Modify: `caliban/Cargo.toml`
- Modify: `caliban/src/main.rs`

- [ ] **Step 1: Cargo.toml**

Add to `[dependencies]`:

```toml
caliban-sessions = { path = "../crates/caliban-sessions" }
```

- [ ] **Step 2: Args struct additions**

Add to `Args`:

```rust
/// Load or create a named session; persists to ~/.local/share/caliban/sessions/<NAME>.json.
#[arg(long, value_name = "NAME")]
session: Option<String>,

/// Don't save the session back to disk after the run.
#[arg(long)]
no_save: bool,

/// Override the sessions directory.
#[arg(long, value_name = "DIR")]
sessions_dir: Option<PathBuf>,
```

- [ ] **Step 3: Wire session loading + saving in `main`**

Currently `main` builds `messages = vec![Message::user_text(prompt)]`. Replace with:

```rust
use caliban_sessions::{PersistedSession, SessionStore};

// Resolve store
let store = match (&args.sessions_dir, &args.session) {
    (_, None) => None,
    (Some(d), Some(_)) => Some(SessionStore::new(d.clone())),
    (None, Some(_)) => Some(SessionStore::new(SessionStore::default_root()?)),
};

// Load or create session
let mut session = if let (Some(store), Some(name)) = (&store, &args.session) {
    Some(match store.load(name)? {
        Some(existing) => existing,
        None => PersistedSession::new(name.clone(), provider_name(args.provider), model.clone()),
    })
} else {
    None
};

// Build initial messages: prior session history + new user prompt
let mut messages = session.as_ref().map(|s| s.messages.clone()).unwrap_or_default();
messages.push(Message::user_text(prompt));

// ... run agent ...

// After: save back
if let (Some(store), Some(mut s)) = (store.as_ref(), session.take()) {
    if !args.no_save {
        // Get the outcome from the stream — but the stream API runs until completion;
        // we need to accumulate Final messages + Usage. Move that accumulation
        // outside the existing loop or use Agent::run_until_done (non-streaming) for sessions.
        //
        // Simplest fix: switch session-mode to use run_until_done() and feed
        // outcome.final_messages + outcome.total_usage into session.merge_run.
        s.merge_run(final_messages, total_usage);
        store.save(&s)?;
    }
}
```

In practice the cleanest implementation refactors `main` into two paths:
- `run_streaming_render` — uses `stream_until_done`, renders to terminal. Returns `(final_messages, total_usage, stopped_for)` after consuming the stream.
- `main` calls `run_streaming_render` and (if `--session`) updates and saves the session.

To extract `final_messages` and `total_usage` from a streaming run, accumulate them as you process events:
- Track `final_messages` from the last `TurnEvent::RunEnd { final_messages, total_usage, ... }`.
- That event is always the last; consuming it gives the full picture.

- [ ] **Step 4: Refactor `main` into smaller pieces**

Suggested factoring (the implementer should choose between this and inline):

```rust
async fn run_and_render(
    agent: Arc<Agent>,
    messages: Vec<Message>,
    cancel: CancellationToken,
    args: &Args,
) -> Result<(Vec<Message>, Usage)> {
    let mut stream = Arc::clone(&agent).stream_until_done(messages, cancel);
    let mut tool_inputs: HashMap<String, String> = HashMap::new();
    let mut at_column_zero = true;
    let mut final_messages = None;
    let mut total_usage = Usage::default();

    while let Some(event) = stream.next().await {
        match event? {
            // ... existing match arms ...
            TurnEvent::RunEnd { final_messages: fm, total_usage: tu, turn_count, .. } if !args.quiet => {
                if !at_column_zero { println!(); }
                eprintln!("\n[caliban: {turn_count} turns · {}↑ {}↓ tokens]", tu.input_tokens, tu.output_tokens);
                final_messages = Some(fm);
                total_usage = tu;
            }
            TurnEvent::RunEnd { final_messages: fm, total_usage: tu, .. } => {
                final_messages = Some(fm);
                total_usage = tu;
            }
            // ...
        }
    }
    if !at_column_zero { println!(); }
    Ok((final_messages.unwrap_or_default(), total_usage))
}
```

Then `main` flow:

```rust
let (final_messages, total_usage) = run_and_render(agent, messages, cancel, &args).await?;
if let (Some(store), Some(mut session)) = (store.as_ref(), session.take()) {
    if !args.no_save {
        session.merge_run(final_messages, total_usage);
        store.save(&session)?;
        if !args.quiet {
            eprintln!("[caliban: saved session '{}' ({} turns, {} tokens)]",
                session.name, session.turn_count(),
                session.total_usage.input_tokens + session.total_usage.output_tokens);
        }
    }
}
```

- [ ] **Step 5: Verify**

```bash
cargo build  --bin caliban
cargo test   -p caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
./target/debug/caliban --help   # should show --session, --no-save, --sessions-dir
```

- [ ] **Step 6: Commit**

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(cli): --session flag for persistent single-prompt conversations

Single-prompt invocations with --session <name> load the session's
prior history, append the new user prompt, run the agent, and save
the updated history+usage back. New sessions are auto-created.
--no-save runs without writing back. --sessions-dir overrides the
storage root (default: ~/.local/share/caliban/sessions/).

Refactors the event loop into run_and_render which returns
(final_messages, total_usage) for session bookkeeping.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Interactive REPL

**Files:**
- Modify: `caliban/Cargo.toml` — add `rustyline = "14"`
- Create: `caliban/src/repl.rs`
- Modify: `caliban/src/main.rs` — dispatch to REPL when no prompt + TTY

- [ ] **Step 1: Cargo.toml — add rustyline**

```toml
[dependencies]
rustyline = "14"
```

- [ ] **Step 2: Detect REPL conditions in main**

```rust
use std::io::IsTerminal;

let should_repl = args.prompt.is_none() && args.prompt_flag.is_none() && std::io::stdin().is_terminal();
if should_repl {
    return repl::run(args, /* ... */).await;
}
```

- [ ] **Step 3: `caliban/src/repl.rs`**

```rust
//! Interactive REPL mode.

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result};
use caliban_agent_core::{Agent, Message, TurnEvent};
use caliban_provider::Usage;
use caliban_sessions::{PersistedSession, SessionStore};
use futures::StreamExt;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};
use tokio_util::sync::CancellationToken;

use crate::{summarize, summarize_blocks, Args};

enum CommandOutcome {
    Continue,
    Exit,
    RunPrompt(String),
}

/// Run the REPL loop.
///
/// # Errors
/// Propagates from agent runs and session-store I/O.
pub async fn run(
    args: Args,
    agent: Arc<Agent>,
    store: Option<SessionStore>,
    mut session: Option<PersistedSession>,
    cancel: CancellationToken,
) -> Result<()> {
    let history_path = dirs::data_dir()
        .map(|d| d.join("caliban").join("repl_history.txt"));

    let mut rl: Editor<(), FileHistory> = Editor::with_config(
        Config::builder().auto_add_history(true).build(),
    )?;
    if let Some(p) = &history_path {
        let _ = rl.load_history(p);  // best effort
    }

    print_banner(&args, session.as_ref());

    loop {
        let line = match rl.readline("> ") {
            Ok(line) => line,
            Err(ReadlineError::Eof) => break,
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C at prompt: exit
                break;
            }
            Err(e) => return Err(e.into()),
        };

        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        if trimmed.starts_with('/') {
            match handle_command(trimmed, &store, session.as_mut()) {
                CommandOutcome::Continue => continue,
                CommandOutcome::Exit => break,
                CommandOutcome::RunPrompt(p) => {
                    run_one_turn(&agent, &mut session, &args, &cancel, &p).await?;
                }
            }
        } else {
            run_one_turn(&agent, &mut session, &args, &cancel, trimmed).await?;
        }

        // Save session after each turn if persistent
        if let (Some(store), Some(s)) = (store.as_ref(), session.as_ref()) {
            if !args.no_save {
                let _ = store.save(s);  // best effort; print error to stderr if it fails
            }
        }
    }

    if let Some(p) = &history_path {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let _ = rl.save_history(p);
    }

    if let (Some(store), Some(s)) = (store.as_ref(), session.as_ref()) {
        if !args.no_save {
            store.save(s).context("save session on exit")?;
            if !args.quiet {
                eprintln!("[caliban: saved session '{}']", s.name);
            }
        }
    }

    Ok(())
}

fn print_banner(args: &Args, session: Option<&PersistedSession>) {
    let version = env!("CARGO_PKG_VERSION");
    let provider = format!("{:?}", args.provider).to_lowercase();
    let model = args.model.as_deref().unwrap_or_else(|| crate::default_model_for(args.provider));
    let session_info = session.map(|s| format!(" — session: {} ({} turns, {}k tokens)",
        s.name,
        s.turn_count(),
        (s.total_usage.input_tokens + s.total_usage.output_tokens) / 1000)).unwrap_or_default();
    println!("caliban v{version} — {provider} {model}{session_info}");
    println!("Type your message; /help for commands; /exit or Ctrl-D to quit.");
    println!();
}

fn handle_command(
    line: &str,
    store: &Option<SessionStore>,
    session: Option<&mut PersistedSession>,
) -> CommandOutcome {
    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match cmd {
        "/help" => {
            println!("Commands:");
            println!("  /help                — show this help");
            println!("  /exit, /quit         — save and exit");
            println!("  /clear               — clear in-memory history (does not save)");
            println!("  /sessions            — list saved sessions");
            println!("  /load <name>         — load a named session");
            println!("  /save [<name>]       — save current session (optionally rename)");
            println!("  /usage               — show accumulated usage");
            println!("Anything not starting with / is sent as a user prompt.");
            CommandOutcome::Continue
        }
        "/exit" | "/quit" => CommandOutcome::Exit,
        "/clear" => {
            if let Some(s) = session {
                s.messages.clear();
                println!("[history cleared]");
            } else {
                println!("[no session active]");
            }
            CommandOutcome::Continue
        }
        "/sessions" => {
            match store {
                Some(s) => match s.list() {
                    Ok(list) if list.is_empty() => println!("[no sessions yet]"),
                    Ok(list) => {
                        for m in list {
                            println!("  {} — {} turns, {} tokens — {}",
                                m.name, m.turn_count, m.total_tokens, m.updated_at.format("%Y-%m-%d %H:%M:%S"));
                        }
                    }
                    Err(e) => println!("[error listing: {e}]"),
                },
                None => println!("[no session store]"),
            }
            CommandOutcome::Continue
        }
        "/usage" => {
            match session {
                Some(s) => println!("session {}: {} turns, {} input + {} output tokens",
                    s.name, s.turn_count(), s.total_usage.input_tokens, s.total_usage.output_tokens),
                None => println!("[no session active]"),
            }
            CommandOutcome::Continue
        }
        "/save" => {
            if let (Some(store), Some(s)) = (store, session) {
                let target_name = if arg.is_empty() { s.name.clone() } else { arg.to_string() };
                if target_name != s.name {
                    let mut renamed = s.clone();
                    renamed.name = target_name;
                    match store.save(&renamed) {
                        Ok(()) => println!("[saved as '{}']", renamed.name),
                        Err(e) => println!("[save error: {e}]"),
                    }
                } else {
                    match store.save(s) {
                        Ok(()) => println!("[saved]"),
                        Err(e) => println!("[save error: {e}]"),
                    }
                }
            } else {
                println!("[no session to save]");
            }
            CommandOutcome::Continue
        }
        "/load" => {
            // For simplicity: print that this would reload. Real implementation
            // would require swapping `session` and reinitializing the REPL state.
            // V1 limitation: documented; user can /exit and reinvoke with --session <name>.
            println!("[/load not yet implemented in v1; /exit and reinvoke with --session {arg}]");
            CommandOutcome::Continue
        }
        unknown => {
            println!("Unknown command: {unknown}. Type /help.");
            CommandOutcome::Continue
        }
    }
}

async fn run_one_turn(
    agent: &Arc<Agent>,
    session: &mut Option<PersistedSession>,
    args: &Args,
    cancel: &CancellationToken,
    prompt: &str,
) -> Result<()> {
    let mut messages: Vec<Message> = session.as_ref()
        .map(|s| s.messages.clone())
        .unwrap_or_default();
    messages.push(Message::user_text(prompt.to_string()));

    let mut stream = Arc::clone(agent).stream_until_done(messages, cancel.clone());

    let mut tool_inputs: HashMap<String, String> = HashMap::new();
    let mut at_column_zero = true;
    let mut final_messages: Vec<Message> = Vec::new();
    let mut total_usage = Usage::default();

    while let Some(event) = stream.next().await {
        match event {
            Err(e) if matches!(e, caliban_agent_core::Error::Cancelled) => {
                eprintln!("\n[cancelled]");
                return Ok(());  // back to prompt
            }
            Err(e) => {
                eprintln!("\n[error: {e}]");
                return Ok(());  // back to prompt; don't propagate (REPL keeps running)
            }
            Ok(event) => {
                match event {
                    TurnEvent::AssistantTextDelta { text, .. } => {
                        print!("{text}");
                        std::io::stdout().flush().ok();
                        at_column_zero = text.ends_with('\n');
                    }
                    TurnEvent::AssistantThinkingDelta { text, .. } if !args.quiet => {
                        eprint!("\x1b[2m{text}\x1b[0m");
                    }
                    TurnEvent::ToolCallStart { tool_use_id, name, .. } if !args.quiet => {
                        if !at_column_zero { eprintln!(); }
                        tool_inputs.insert(tool_use_id, String::new());
                        eprint!("\u{1F527} {name}(");
                    }
                    TurnEvent::ToolCallInputDelta { tool_use_id, partial_json, .. } => {
                        tool_inputs.entry(tool_use_id).or_default().push_str(&partial_json);
                    }
                    TurnEvent::ToolCallEnd { tool_use_id, is_error, content, .. } if !args.quiet => {
                        let input_str = tool_inputs.remove(&tool_use_id).unwrap_or_default();
                        let input_summary = summarize(&input_str, 80);
                        let result_summary = summarize_blocks(&content, 80);
                        let prefix = if is_error { "(error) " } else { "" };
                        eprintln!("{input_summary})");
                        eprintln!("   \u{2192} {prefix}{result_summary}");
                        at_column_zero = true;
                    }
                    TurnEvent::RunEnd { final_messages: fm, total_usage: tu, turn_count, .. } => {
                        if !at_column_zero { println!(); }
                        if !args.quiet {
                            eprintln!("[caliban: {turn_count} turns · {}\u{2191} {}\u{2193} tokens]",
                                tu.input_tokens, tu.output_tokens);
                        }
                        final_messages = fm;
                        total_usage = tu;
                        at_column_zero = true;
                    }
                    _ => {}
                }
            }
        }
    }

    if !at_column_zero { println!(); }

    if let Some(s) = session {
        s.merge_run(final_messages, total_usage);
    }

    Ok(())
}
```

- [ ] **Step 4: Wire REPL dispatch in main**

In `main`, after constructing `agent` and (optionally) `session`:

```rust
let has_prompt = args.prompt.is_some() || args.prompt_flag.is_some();
let stdin_is_tty = std::io::stdin().is_terminal();

if !has_prompt && stdin_is_tty {
    return repl::run(args, agent, store, session, cancel).await;
}

// ... existing single-prompt code path ...
```

Make sure `summarize` and `summarize_blocks` are `pub` (or `pub(crate)`) so `repl.rs` can call them. The `Args` struct also needs `pub` for the field access from `repl.rs`.

- [ ] **Step 5: Verify**

```bash
cargo build  --bin caliban
cargo test   -p caliban
cargo clippy -p caliban --all-targets -- -D warnings
```

Manual test: `./target/debug/caliban` (no args, in a TTY) — should enter the REPL banner. `/help` shows commands. `/exit` exits cleanly.

(Don't run a live model in the automated verification; the implementer should describe what they tested manually.)

- [ ] **Step 6: Commit**

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
feat(cli): interactive REPL mode

`caliban` with no prompt + a TTY stdin enters an interactive REPL.
rustyline provides line editing + ~/.local/share/caliban/repl_history.txt
persistence. Slash commands /help, /exit, /quit, /clear, /sessions,
/save, /usage are recognized; unknown commands print a hint.

REPL is session-aware: with --session <name>, every turn is auto-saved
back to disk. Without --session, the REPL is in-memory-only.

Ctrl+C during a turn cancels that turn (returns to prompt). Ctrl+C
at the prompt exits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: ADR 0011 + README update

- Create `docs/adr/0011-sessions-and-repl.md` capturing: session-as-file (vs sqlite), name-validation regex, REPL-as-additive-feature, slash-command syntax.
- Append `0011` row to `docs/adr/README.md` index.
- Update root README:
  - Note Sessions + REPL in the "Project status" callout.
  - Add `caliban-sessions/` to repo layout.
  - New section "Sessions and REPL" with examples.

Commit: `docs: ADR 0011 + README for sessions + REPL`

---

## Self-Review

All four tasks are independent and ordered by dependency: Task 1 ships the crate; Task 2 wires `--session` for single-prompt; Task 3 wires REPL; Task 4 documents. The plan's biggest risk is the REPL implementation (Task 3) — `rustyline` integration + the slash-command handler + the embedded run-one-turn function are ~300 lines. Single-file in `repl.rs` keeps it manageable. All public types from `caliban-sessions` are object-safe — no `dyn` complications. Tests in Task 1 cover the round-trip; the REPL test is manual (running in a TTY).
