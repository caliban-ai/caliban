# Context Preservation + Path Fixes · Design

- **Date:** 2026-05-23
- **Status:** Draft
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** TUI fixes + system prompt sub-project

## Goals

Four fixes bundled together, all surfaced by real-use testing:

1. **Ephemeral REPL loses history between turns.** Without `--session`, every Enter in the TUI starts a fresh conversation — the model only sees the system prompt + the new user message. This is a bug; `--session` mode works correctly because it persists through the session store, but the ephemeral path drops `final_messages` on the floor after each `RunEnd`.

2. **`~` not expanded in tool paths.** When the model calls a tool with a path like `~/dev` or sets Bash's `cwd` to `~/dev`, our `WorkspaceRoot::resolve` treats `~` as a literal character. The OS then fails with "no such file or directory." The model interprets this error as "the directory doesn't exist" and gives up.

3. **TUI tool-input summary is misleading.** During streaming, partial JSON deltas accumulate. The current `ToolCallEnd` rendering truncates the first 80 chars of the raw partial-JSON string, which can leave the display showing things like `{"path": "/Users/.../cs-5254", "pattern": "*` — looking like the pattern is just `*` when it's actually `**/*.rs`. The accumulated JSON is COMPLETE at `ToolCallEnd`; we should parse it and render the actual field values.

4. **System prompt doesn't mention path conventions.** Adding a sentence telling the model that paths can use `~` (which we now handle), or absolute paths, or paths relative to the working directory.

## Acceptance criteria

- In the TUI without `--session`, asking a follow-up question references prior turns correctly. The model knows what tool it just called and what the result was.
- The agent can call `Bash` with `cwd: "~/dev"` and it works (resolves to `$HOME/dev`).
- The agent can call `Read({"path": "~/notes.md"})` and it works.
- The TUI tool-input summary at `ToolCallEnd` shows parsed JSON field values, not raw partial text. Long values are truncated cleanly (with `…`).
- The default system prompt mentions that `~` is supported in tool path arguments.
- All existing tests pass; new tests cover tilde-expansion behavior.

## Non-goals

- Shell variable expansion (`$HOME`, `${VAR}`) — only `~` and `~/`. Users can put env-var-using commands in the Bash `command` field where the shell handles them.
- Tilde-user expansion (`~john`, `~root`) — only bare `~` and `~/`. Standard POSIX-ish tilde-user is rare in modern tooling; defer.
- Compacting / token-counting based on the new in-memory message buffer. Existing compactor still applies via agent-core; that's unchanged.
- Showing the full conversation log in the TUI (more than the existing transcript). The bug is about *agent* memory, not user-visible display.

## Design

### 1. Ephemeral REPL message history

Add to `App` (in `caliban/src/tui.rs`):

```rust
pub(crate) struct App {
    // ... existing fields ...
    pub(crate) messages: Vec<Message>,
}
```

Initialization in `App::new`:

```rust
let messages = session.as_ref()
    .map(|s| s.messages.clone())
    .unwrap_or_default();
```

In the Enter handler in `handle_key`, replace:

```rust
let mut messages: Vec<Message> = app.session.as_ref()
    .map(|s| s.messages.clone())
    .unwrap_or_default();
// ... system injection ...
messages.push(Message::user_text(prompt_text));
```

with:

```rust
let mut messages = app.messages.clone();
// system injection (only on first turn — if messages is empty or lacks Role::System):
let has_system = messages.first().is_some_and(|m| m.role == caliban_provider::Role::System);
if !has_system {
    if let Some(p) = &app.system_prompt {
        messages.insert(0, Message::system_text(p.clone()));
    }
}
messages.push(Message::user_text(prompt_text));
```

In `handle_agent_event`'s `RunEnd` arm:

```rust
TurnEvent::RunEnd { final_messages, total_usage, turn_count, .. } => {
    // Update the in-memory history
    app.messages = final_messages.clone();
    // Also update session if persistent
    if let Some(sess) = app.session.as_mut() {
        sess.merge_run(final_messages, total_usage);
        // ... existing save logic ...
    }
    // ... existing usage-summary line ...
}
```

This means:
- Ephemeral REPL: history grows in `app.messages` across turns.
- Session mode: `app.messages` and `app.session.messages` stay in sync.
- `/clear` slash command: clears both `app.messages` and the transcript display.

Also update `/clear`:

```rust
"/clear" => {
    app.transcript.clear();
    app.messages.clear();
    // Don't clear session.messages — that's the persistent state.
    // Or DO clear it? Discuss in spec.
}
```

**Question:** should `/clear` also clear `app.session.messages`? Two interpretations:
- (a) `/clear` clears the visible transcript only; underlying session is preserved.
- (b) `/clear` is a "start over" command that resets everything.

For v1 go with **(b)**: `/clear` wipes both the in-memory history AND the session's messages (next save will overwrite). The user can `/save <new-name>` first if they want a snapshot.

### 2. `~` expansion in `WorkspaceRoot::resolve`

Modify `crates/caliban-tools-builtin/src/workspace.rs::resolve`:

```rust
pub fn resolve(&self, input: &str) -> Result<PathBuf, ToolError> {
    if input.is_empty() {
        return Err(ToolError::invalid_input("empty path"));
    }

    // Expand leading ~ to the user's home directory.
    let candidate: PathBuf = if input == "~" {
        dirs::home_dir().ok_or_else(|| ToolError::invalid_input("~ used but home dir unknown"))?
    } else if let Some(rest) = input.strip_prefix("~/") {
        let mut home = dirs::home_dir().ok_or_else(|| ToolError::invalid_input("~/ used but home dir unknown"))?;
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

Add `dirs = { workspace = true }` to `caliban-tools-builtin/Cargo.toml` (already in workspace deps from earlier).

Tests in `workspace.rs` mod tests:
- `resolve_tilde_only` — `~` resolves to home dir.
- `resolve_tilde_path` — `~/foo` resolves to `$HOME/foo`.
- `resolve_tilde_in_restricted_mode_outside_root_rejected` — `~/foo` outside root + restricted → error.
- `resolve_tilde_inside_root_in_restricted_mode_allowed` — if workspace root is `$HOME/work`, then `~/work/file.txt` should resolve correctly and be allowed.

### 3. TUI tool-input summary

In `caliban/src/tui.rs`, the rendering of `ToolCall.input` currently uses `summarize(&input, 80)` which truncates raw text. Replace with a JSON-aware formatter.

In `render_transcript`'s `ToolCall` arm (or wherever the tool call line is built), parse `input` as JSON. If parse succeeds, format as `key=value, key=value` (Python-dict-like brevity); if parse fails, fall back to current raw truncation.

Helper:

```rust
fn format_tool_input(input: &str, max_chars: usize) -> String {
    use serde_json::Value;
    match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(map)) => {
            let mut parts = Vec::with_capacity(map.len());
            for (k, v) in &map {
                let v_str = match v {
                    Value::String(s) => {
                        // Truncate long strings within the value
                        if s.chars().count() > 40 {
                            format!("\"{}…\"", s.chars().take(40).collect::<String>())
                        } else {
                            format!("\"{s}\"")
                        }
                    }
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Null => "null".into(),
                    Value::Array(_) | Value::Object(_) => v.to_string(),  // compact JSON
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
            // Fall back to raw text truncation
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

Render call: `format_tool_input(input, 80)` where the original `summarize` was called.

### 4. System prompt path note

Add one bullet to the default system prompt:

```
- Path arguments to tools support absolute paths, paths relative to the
  working directory above, and `~`/`~/...` for the home directory.
```

(This sits inside the `Conventions:` block.)

## Crate changes

- `crates/caliban-tools-builtin/Cargo.toml`: add `dirs = { workspace = true }` if not already a direct dep (check — it's in workspace.dependencies).
- `crates/caliban-tools-builtin/src/workspace.rs`: implement tilde expansion + tests.
- `caliban/src/tui.rs`: add `App::messages`, update Enter handler + RunEnd handler + `/clear`. Add `format_tool_input` helper. Switch the ToolCall input rendering.
- `caliban/src/system_prompt.rs`: add the path-conventions bullet.

## Acceptance criteria

(Repeated from Goals + measurable.)

- `cargo test -p caliban-tools-builtin` includes 4 new tests for tilde expansion; all pass.
- `cargo test --workspace` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- Manual: `caliban` in REPL → ask a question that needs a follow-up. Submit the follow-up. The model references the prior turn. (Hard to automate; document expected behavior.)
- Manual: model can use `~/foo` in tool paths without errors.
- Manual: TUI's tool-call line shows `pattern="**/*.rs", path="/Users/.../cs-5254"` style instead of truncated partial JSON.

## Risks

- **Tilde in restricted mode:** restricted-mode resolution now operates on the expanded path. If a user's `restrict-paths` setting expected `~` to be outside the root, this is a behavior change. Mitigation: tilde expansion happens BEFORE the restriction check, so the check sees the actual destination. If the user's home is outside the workspace root, restricted mode correctly rejects `~/anything`.
- **App::messages double-keeping with session.messages:** in `--session` mode we now have two copies. Keep them in sync at `RunEnd`. If they diverge due to a bug, the session is the source of truth on next load. Document this invariant in code comments.
- **Pretty-printed input might be slower with very large tool inputs.** Parsing 100KB of accumulated JSON to render a line is wasted work. Mitigation: truncate first (take ~2KB of the input), then attempt parse; if it parses, great; else fall back. Probably overkill for v1; revisit if profiling shows it.
- **`/clear` wiping session.messages on disk:** if user `/clear`s a session by mistake, the next save (auto-save after a turn or on exit) writes the cleared state. They can `/save <new-name>` first to snapshot, but there's no undo. Documented in `/help`.
