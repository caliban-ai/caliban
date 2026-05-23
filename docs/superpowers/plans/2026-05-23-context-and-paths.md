# Context Preservation + Path Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Fix four real-use issues in one sprint:
1. Ephemeral REPL preserves message history across turns within one invocation.
2. `~` is expanded in tool paths.
3. TUI tool-input summary parses JSON instead of showing raw partials.
4. System prompt mentions path conventions (incl. `~`).

**Spec:** [`docs/superpowers/specs/2026-05-23-context-and-paths-design.md`](../specs/2026-05-23-context-and-paths-design.md)

---

## Task W.1: `~` expansion in `WorkspaceRoot::resolve`

**Files:**
- Modify: `crates/caliban-tools-builtin/Cargo.toml` (ensure `dirs` is declared)
- Modify: `crates/caliban-tools-builtin/src/workspace.rs`

- [ ] **Step 1: Add `dirs` to caliban-tools-builtin/Cargo.toml**

Check whether it's already in `[dependencies]`. If not, add:

```toml
dirs = { workspace = true }
```

(`dirs` is already in workspace.dependencies; just reference it.)

- [ ] **Step 2: Update `resolve` to handle `~` and `~/`**

In `crates/caliban-tools-builtin/src/workspace.rs`, replace the body of `resolve`:

```rust
pub fn resolve(&self, input: &str) -> Result<PathBuf, ToolError> {
    if input.is_empty() {
        return Err(ToolError::invalid_input("empty path"));
    }

    // Expand leading ~ or ~/ to the user's home directory.
    let candidate: PathBuf = if input == "~" {
        dirs::home_dir().ok_or_else(|| ToolError::invalid_input("~ used but home directory is unavailable"))?
    } else if let Some(rest) = input.strip_prefix("~/") {
        let mut home = dirs::home_dir().ok_or_else(|| ToolError::invalid_input("~/ used but home directory is unavailable"))?;
        home.push(rest);
        home
    } else {
        PathBuf::from(input)
    };

    let abs = if candidate.is_absolute() {
        candidate
    } else {
        self.root.join(&candidate)
    };

    let canon = canonicalize_existing_ancestor(&abs);
    if self.restrict_to_root && !canon.starts_with(&self.root) {
        return Err(ToolError::invalid_input(format!(
            "path {} is outside workspace root {}",
            canon.display(),
            self.root.display(),
        )));
    }
    Ok(canon)
}
```

- [ ] **Step 3: Add 4 new tests in the `tests` mod**

```rust
#[test]
fn resolve_tilde_only() {
    let tmp = TempDir::new().unwrap();
    let root = WorkspaceRoot::new(tmp.path());
    let resolved = root.resolve("~").unwrap();
    if let Some(home) = dirs::home_dir() {
        // canonicalize_existing_ancestor may collapse symlinks
        let expected = std::fs::canonicalize(&home).unwrap_or(home);
        assert_eq!(resolved, expected);
    }
}

#[test]
fn resolve_tilde_path() {
    let tmp = TempDir::new().unwrap();
    let root = WorkspaceRoot::new(tmp.path());
    let resolved = root.resolve("~/foo.txt").unwrap();
    if let Some(home) = dirs::home_dir() {
        let canon_home = std::fs::canonicalize(&home).unwrap_or(home);
        assert_eq!(resolved, canon_home.join("foo.txt"));
    }
}

#[test]
fn resolve_tilde_in_restricted_mode_outside_root_rejected() {
    let tmp = TempDir::new().unwrap();
    let root = WorkspaceRoot::new(tmp.path()).restricted();
    // ~ resolves outside the tempdir; restricted mode should reject.
    let err = root.resolve("~/notes.md").unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
}

#[test]
fn resolve_no_tilde_unchanged() {
    let tmp = TempDir::new().unwrap();
    let root = WorkspaceRoot::new(tmp.path());
    let resolved = root.resolve("subdir/file.txt").unwrap();
    assert!(resolved.starts_with(root.root()));
    assert!(resolved.ends_with("subdir/file.txt"));
}
```

- [ ] **Step 4: Build + test + commit**

```bash
cargo test -p caliban-tools-builtin
cargo clippy -p caliban-tools-builtin --all-targets -- -D warnings
cargo fmt --all -- --check
```

All exit 0; the 4 new tests pass.

```bash
git add crates/caliban-tools-builtin/
git commit -m "$(cat <<'EOF'
fix(tools-builtin): expand ~ in WorkspaceRoot::resolve

Path arguments like ~/dev or ~ are now expanded to the user's home
directory before relative-resolution and restriction checks. Tools
calling resolve (Read, Write, Edit, Bash cwd, Glob path, Grep path)
all benefit transparently.

Resolves the "Error: No such file or directory" that occurred when
the model passed paths like cwd:"~/dev" to Bash — Rust's
Command::current_dir doesn't shell-expand tildes.

The Bash command field (the actual script string) is unchanged; the
shell handles ~ expansion there as it always has.

Tests cover ~ alone, ~/path, restricted-mode rejection, and
non-tilde unchanged behavior.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task W.2: Ephemeral REPL message history

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Add `messages: Vec<Message>` to `App`**

Find the `App` struct definition. Add the field:

```rust
pub(crate) struct App {
    // ... existing fields ...
    pub(crate) messages: Vec<caliban_provider::Message>,
}
```

Order it after `session` for clarity.

- [ ] **Step 2: Initialize in `App::new`**

After computing the existing fields, add:

```rust
let messages = session
    .as_ref()
    .map(|s| s.messages.clone())
    .unwrap_or_default();
```

And include `messages` in the struct literal.

- [ ] **Step 3: Update Enter-handler message construction**

Find the spot in `handle_key`'s Enter arm that builds `messages` for the turn. Replace:

```rust
let mut messages = app.session.as_ref()
    .map(|s| s.messages.clone())
    .unwrap_or_default();
```

with:

```rust
let mut messages = app.messages.clone();
```

The system-prompt injection logic that follows stays unchanged (it checks `messages.first()` for an existing Role::System).

- [ ] **Step 4: Update `handle_agent_event`'s RunEnd arm**

In the `TurnEvent::RunEnd` arm, store the new history on `app`:

```rust
TurnEvent::RunEnd { final_messages, total_usage, turn_count, .. } => {
    // Update the in-memory history (works for both ephemeral and session modes).
    app.messages = final_messages.clone();

    // ... existing transcript usage-summary push ...

    // Persist to session if applicable
    if let Some(sess) = app.session.as_mut() {
        sess.merge_run(final_messages, total_usage);
        if let Some(store) = app.store.as_ref() {
            if !app.args.no_save {
                if let Err(e) = store.save(sess) {
                    app.transcript.push(TranscriptLine::Error(format!("save failed: {e}")));
                } else {
                    app.transcript.push(TranscriptLine::Info("session saved".into()));
                }
            }
        }
    }
    app.running = None;
    app.auto_scroll = true;
}
```

(The `final_messages.clone()` happens before `session.merge_run` consumes it; adjust the order so `app.messages = final_messages` happens AFTER `merge_run` if that consumes by value. Alternatively `final_messages` is `Vec<Message>` which is moved; clone once and pass owned to both. The cleanest order:)

```rust
TurnEvent::RunEnd { final_messages, total_usage, turn_count, .. } => {
    // ... transcript push for usage summary ...

    // Update in-memory history.
    app.messages = final_messages.clone();

    // Persist to session if applicable (consumes final_messages).
    if let Some(sess) = app.session.as_mut() {
        sess.merge_run(final_messages, total_usage);
        // ... save logic ...
    }

    app.running = None;
    app.auto_scroll = true;
}
```

- [ ] **Step 5: Update `/clear` slash command**

Find `handle_slash_command`'s `/clear` arm:

```rust
"/clear" => {
    app.transcript.clear();
    app.messages.clear();
    // Clear session messages too if applicable; the next save overwrites.
    if let Some(sess) = app.session.as_mut() {
        sess.messages.clear();
    }
}
```

Update the `/help` overlay's `/clear` description to mention this:

```rust
("/clear", "Clear transcript AND in-memory history (session messages cleared too)"),
```

- [ ] **Step 6: Build + test + commit**

```bash
cargo build  --bin caliban
cargo test   -p caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
fix(tui): ephemeral REPL preserves message history across turns

App now carries an in-memory messages: Vec<Message> field that's
the source of truth for the next turn's context. Initialized from
session.messages on startup (or empty); updated from final_messages
at each RunEnd; cleared by /clear (which now also wipes session
messages).

Previously, ephemeral REPL mode (no --session) had no memory between
turns — each Enter built a 2-message list (system + new user prompt)
and dropped final_messages on the floor. Now history accumulates the
same way --session does.

Session mode is unaffected behaviorally (already worked); the
in-memory buffer just stays in sync with session.messages.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task W.3: TUI tool-input summary via JSON parse

**Files:** modify `caliban/src/tui.rs`.

- [ ] **Step 1: Add `format_tool_input` helper**

Near the existing `summarize`/`summarize_blocks` helpers (or in a sensible spot in tui.rs), add:

```rust
fn format_tool_input(input: &str, max_chars: usize) -> String {
    use serde_json::Value;
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(map)) => {
            let mut parts: Vec<String> = Vec::with_capacity(map.len());
            for (k, v) in &map {
                let v_str = match v {
                    Value::String(s) => {
                        if s.chars().count() > 40 {
                            let truncated: String = s.chars().take(40).collect();
                            format!("\"{truncated}…\"")
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
                format!("{truncated}…")
            } else {
                joined
            }
        }
        _ => {
            if input.chars().count() > max_chars {
                let truncated: String = input.chars().take(max_chars).collect();
                format!("{truncated}…")
            } else {
                input.to_string()
            }
        }
    }
}
```

- [ ] **Step 2: Replace the raw-text summarize call in the ToolCall rendering**

Find `render_transcript`'s `TranscriptLine::ToolCall` arm. The line that builds the input summary (currently calls `summarize` or inlines `input.chars().take(80)`). Replace with:

```rust
let input_summary = format_tool_input(input, 80);
```

Result summary (`result_text`) keeps using the existing `summarize` since it's plain text, not JSON.

- [ ] **Step 3: Build + test + commit**

```bash
cargo build  --bin caliban
cargo test   -p caliban
cargo clippy -p caliban --all-targets -- -D warnings
cargo fmt --all -- --check
```

```bash
git add caliban/
git commit -m "$(cat <<'EOF'
fix(tui): parse tool input JSON for cleaner ToolCallEnd rendering

ToolCallEnd now formats the accumulated input as 'key=value, key=value'
pairs by serde-parsing the full JSON (which is complete at end-of-tool-
call). Long string values are truncated within the value (40 chars +
…). Falls back to raw truncation if JSON parse fails.

Previously the summary was raw-text truncation of the partial-JSON
stream — could elide the closing brace and make 'pattern: \"**/*.rs\"'
look like 'pattern: \"*' (just an asterisk). Real-use confusion when
the model dispatched many parallel Glob calls.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task W.4: System prompt path conventions note

**Files:** modify `caliban/src/system_prompt.rs`.

- [ ] **Step 1: Add the bullet**

In `build_default`, find the `Conventions:` block. Add (right after the "File paths can be relative…" line):

```
- Path arguments to tools also support `~` and `~/...` for the home directory.
```

So the conventions block becomes:

```
Conventions:
- Use tools when needed; don't claim to have read files you haven't actually Read.
- File paths can be relative to the working directory above, or absolute.
- Path arguments to tools also support `~` and `~/...` for the home directory.
- Bash commands run with /bin/sh -c and timeout after 60s by default.
- Output is rendered in a terminal UI; prefer concise responses with code blocks for multi-line content rather than long prose paragraphs.
- When the user asks you to modify a file, Read it first so your edits are accurate.
```

- [ ] **Step 2: Build + commit**

```bash
cargo build --bin caliban
git add caliban/
git commit -m "$(cat <<'EOF'
docs(system-prompt): mention ~/ path support in tool arguments

Adds one bullet to the default system prompt's Conventions block
noting that tools support ~ and ~/... in path arguments. Pairs with
the WorkspaceRoot::resolve tilde-expansion fix so the model knows
the feature exists and is reliable.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task W.5: ADR 0015 + README

**Files:**
- Create: `adrs/0015-context-and-path-fixes.md`
- Modify: `adrs/README.md`
- Modify: `README.md`

- [ ] **Step 1: ADR 0015**

```markdown
# ADR 0015 · Context preservation + path conventions (~/dev fix)

- **Status:** accepted
- **Date:** 2026-05-23

## Context

Real-use testing surfaced four issues bundled into one fix:

1. The TUI's ephemeral REPL (no `--session`) silently dropped every
   turn's `final_messages`, so each new prompt only saw the system
   prompt + the latest user message. Models had no memory of prior
   turns in the same REPL session.
2. `WorkspaceRoot::resolve` didn't expand `~`. When models invoked
   `Bash` with `cwd: "~/dev"` or `Read({"path":"~/notes.md"})` the
   path resolution failed with "No such file or directory." The
   model misinterpreted the error as "directory doesn't exist."
3. The TUI's tool-call input summary truncated the partial-JSON stream
   at 80 chars, sometimes hiding closing braces and making patterns
   look different than they were.
4. The default system prompt didn't tell the model that `~` is
   supported in tool paths.

## Decision

1. Add `messages: Vec<Message>` to the TUI's `App`. Initialize from
   session if any, else empty. Update from `RunEnd`'s `final_messages`
   each turn. `/clear` wipes both the in-memory history and the
   session's persisted messages.
2. `WorkspaceRoot::resolve` expands a leading `~` or `~/` to
   `dirs::home_dir()`. Affects all path arguments to all tools.
   The `Bash` command string is unchanged — the shell handles `~`
   expansion there.
3. At `ToolCallEnd`, parse the accumulated input as JSON and render
   `key="value", key=value` pairs. Fall back to raw truncation on
   parse failure.
4. Add a path-conventions bullet to the default system prompt.

## Consequences

- **Positive:** Ephemeral REPL now feels like a real conversation
  rather than a series of disconnected one-shots. `~/foo` paths
  work transparently. Tool-call summaries are readable. The
  system prompt's conventions are accurate.
- **Negative:** `App::messages` and `session.messages` are now two
  copies in `--session` mode (kept in sync at `RunEnd`). `/clear`
  is destructive to session-stored messages — documented.
- **Revisit if:** The double-keeping causes correctness bugs (e.g.,
  divergence after a mid-flight panic). The cleanest long-term
  refactor would be to make `App` hold an `Arc<RwLock<Session>>`
  and treat session as the single source of truth, with the
  ephemeral case using a synthetic in-memory session.
```

- [ ] **Step 2: Update `adrs/README.md`**

Append after ADR 0014's row:

```
| [0015](0015-context-and-path-fixes.md) | Context preservation + path conventions (~ expansion) | accepted |
```

- [ ] **Step 3: Update root `README.md`**

In the "Interactive TUI" section, add a brief note (in the slash-command list or as a paragraph below):

```
The REPL preserves message history across turns, even without
`--session`. Use `--session <name>` to persist that history to disk
between invocations.
```

Update the `/clear` slash-command description to note it wipes history:

```
/clear — clear transcript and in-memory history (also clears session if active)
```

- [ ] **Step 4: Verify + commit**

```bash
cargo fmt --all -- --check
cargo build  --workspace
cargo test   --workspace
cargo clippy --workspace --all-targets -- -D warnings
git add adrs/ README.md
git commit -m "$(cat <<'EOF'
docs: ADR 0015 + README update for context + path fixes

ADR 0015 captures the bundled fix: ephemeral REPL message history,
WorkspaceRoot tilde expansion, JSON-aware tool-input summary, and
the system-prompt path-conventions note. README notes that the REPL
preserves history within an invocation; /clear now also wipes
history.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

Coverage: W.1 (tilde), W.2 (history), W.3 (JSON summary), W.4 (system prompt bullet), W.5 (docs). All four spec goals addressed. Type consistency: `App::messages` defined in W.2 referenced in W.3 (NO — W.3 doesn't touch messages; only tool-call rendering). Helper function `format_tool_input` is a static helper, no state dependency.
