# TodoWrite — Design

**Date:** 2026-05-23
**Status:** Approved
**Target branch:** `jf/feat/todo-write`
**Author:** John Ford
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-agent-core`, `caliban-tools-builtin`, `caliban-cli` (session storage)

## Goal

Add a `TodoWrite` tool to `caliban-tools-builtin` that lets the model
maintain a short, structured list of in-flight tasks across a session.
The list is exposed back to the model in subsequent turns via a small
appended block in the system prompt, so the model sees its own plan
without re-deriving it from the transcript on every turn. This matches
the affordance Claude Code's reference implementation provides and is
the #2 priority quick-win on the post-WebFetch roadmap.

## Non-goals

- **Cross-session todo persistence independent of `Session`.** The list
  lives on the `Session` and rides along in the existing
  `PersistedSession` JSON; ephemeral (no `--session`) runs lose it on
  exit. A future memory tier-1 implementation may persist this layer
  separately — that's a different design.
- **Agent-to-agent shared todos.** Each agent (and each sub-agent, once
  the primitive lands) carries its own list. No cross-agent sync.
- **Per-task ownership semantics** (claim / lock / handoff). That's the
  job of a future `Task` primitive; the `Todo` here is a flat note for
  the model itself.
- **Diff-style updates.** Calling `TodoWrite` REPLACES the entire list.
  This matches Claude Code's choice and avoids the complexity of stable
  ID management, diff merging, and partial-update conflict resolution.
- **Render-side todo widgets in the TUI** beyond a count in the status
  line. The model already shows the list inline when it cares; a
  dedicated overlay is a follow-up.

## Input schema

```json
{
  "type": "object",
  "properties": {
    "todos": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "id":      { "type": "string" },
          "content": { "type": "string" },
          "status":  { "enum": ["pending", "in_progress", "completed", "cancelled"] }
        },
        "required": ["id", "content", "status"]
      }
    }
  },
  "required": ["todos"]
}
```

Semantics:

- The `todos` array REPLACES the entire stored list. Reordering is
  expressed by reordering the array. Deletion is expressed by omitting
  an item from the new array. There is no "patch" form.
- `id` is whatever the model chooses — strings, likely small integers
  rendered as `"1"`, `"2"`, etc. The tool does not enforce uniqueness
  across calls, but it does enforce uniqueness within a single call
  (duplicate `id`s in one payload → `InvalidInput`).
- `content` is a single-line plain string. Newlines are accepted but
  collapsed to spaces when surfaced into the system prompt.
- `status` is a fixed enum. The strings match Claude Code's reference
  set so models trained on that vocabulary work out of the box.

### Constraints

- ≤ 100 todos per list. Larger payloads → `InvalidInput` with a
  message naming the cap. This is a guard against pathological model
  output, not a UX target — typical lists are 3–10 items.
- Each `content` ≤ 500 chars. Oversize → `InvalidInput`.
- IDs ≤ 64 chars. Oversize → `InvalidInput`.

## Output format

The tool returns a single text block in the existing
`→ Header` style used by all other built-ins:

```
→ TodoWrite: 7 total (2 pending, 1 in-progress, 3 completed, 1 cancelled)
```

When the list is empty after a write:

```
→ TodoWrite: list cleared
```

No body content; the model already supplied the full list and doesn't
need it echoed back. Counts go in the header so the model sees that the
write took effect.

## Storage

### In-memory

The agent's `Session` (in `caliban-cli/src/session.rs`) gains a single
new field:

```rust
pub struct Session {
    // ... existing fields ...
    pub todos: Vec<Todo>,
}

pub struct Todo {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

pub enum TodoStatus { Pending, InProgress, Completed, Cancelled }
```

For ephemeral runs (no `--session`), the same `Session` struct is
constructed in-memory and discarded on exit. The tool's behavior is
identical either way.

### On-disk

`PersistedSession` (the JSON written to
`<state_dir>/caliban/sessions/<id>.json`) gains a `"todos"` field that
serializes the `Vec<Todo>`. Loading a session that pre-dates this
change yields an empty `todos` vec via `#[serde(default)]` — no
migration step required.

### Concurrency

Writes to the todo list happen on the agent's main loop, single-
threaded. The `TodoWrite` tool's `invoke()` does not need a mutex; it
gets a `&mut Session` via the same mechanism the (forthcoming)
`Session`-aware tool API uses. **Open question** below: today there is
no `&mut Session` available to a `Tool` — the tool only sees
`ToolContext`. We resolve this by routing the write through a
session-handle indirection (see "Tool lifecycle" below).

## Tool lifecycle

The `Tool` trait today takes a `ToolContext` that exposes a
`tool_use_id` and a `CancellationToken`. It does NOT expose the
`Session`. Two implementation options:

1. **Extend `ToolContext`** with an optional
   `session: Option<Arc<Mutex<Session>>>` field. Tools that need
   per-session state lock briefly; others ignore it. Backwards-
   compatible: existing tools (all of them) don't touch the new field.
2. **Construct `TodoWriteTool` with a session handle.** The tool owns
   an `Arc<Mutex<Session>>` from the moment it's registered, and the
   CLI's `register_tools` wires this up alongside the existing
   `WebFetchTool` wiring.

Choose **(2)**. It keeps the `ToolContext` minimal and doesn't force
every tool author to think about session state. Registry construction
gains one extra line:

```rust
let session = Arc::new(Mutex::new(Session::new(...)));
registry.register(TodoWriteTool::new(Arc::clone(&session)));
// ... agent uses Arc::clone(&session) for its own lookups ...
```

The trade-off: the `caliban-tools-builtin` crate now needs a way to
reference `Session` without depending on `caliban-cli`. We resolve
this by lifting the `Session` + `Todo` types into a new module on
`caliban-agent-core` (or a new tiny `caliban-session` crate if
`agent-core` doesn't want the dep). The CLI layer keeps the persistence
glue. **Open question** below.

## System-prompt surface

When the stored todo list is non-empty, the agent's per-turn system-
prompt builder (in `caliban-cli/src/system_prompt.rs`, post-ADR 0014)
appends a short block to whatever the operator's system prompt is:

```
--- Current todos ---
[ ] (1) draft the WebFetch spec
[~] (2) wire it into the registry
[x] (3) add the cargo dep
[-] (4) decide on summarizer model — abandoned
```

Status glyphs: `[ ]` pending, `[~]` in_progress, `[x]` completed,
`[-]` cancelled. The block is rebuilt each turn (cheap; the list is
small), and the rebuild happens **after** any compactor pass so the
todos don't get dropped along with old history.

When the list is empty, no block is appended. The model gets exactly
what the operator configured.

When `--no-system` is in effect the block is **still** appended (one
line of context for the model is worth more than strict prompt
silence; document this in the flag's help text).

## Crate location

`crates/caliban-tools-builtin/src/todo_write.rs` is the new file
(~250 LOC including tests). It re-exports through
`crates/caliban-tools-builtin/src/lib.rs` alongside `WebFetchTool`:

```rust
pub mod todo_write;
pub use todo_write::TodoWriteTool;
```

Session-state types (`Session`, `Todo`, `TodoStatus`) live in
`caliban-agent-core::session` (new module) so both `tools-builtin` and
`caliban-cli` can reference them without a circular dep.

Cargo dep changes:

- `caliban-tools-builtin/Cargo.toml`: adds nothing new — `serde`,
  `serde_json`, `async-trait`, `tokio` are already in the workspace.
- `caliban-agent-core/Cargo.toml`: unchanged.

## Testing

`#[cfg(test)] mod tests` in `todo_write.rs`:

1. `accepts_empty_list_clears_state` — write `{"todos": []}` → session
   state is empty; output is `list cleared`.
2. `replaces_existing_list_completely` — seed two todos; write a list
   with one different todo; assert only the new todo is present.
3. `preserves_order_from_input` — write three todos in a deliberate
   order; assert `session.todos` matches that order.
4. `rejects_duplicate_ids_in_one_payload` — two items with `id: "1"`
   → `ToolError::InvalidInput`.
5. `rejects_oversize_list` — 101 items → `ToolError::InvalidInput`.
6. `rejects_oversize_content` — one item with 501-char content →
   `ToolError::InvalidInput`.
7. `rejects_oversize_id` — one item with a 65-char id →
   `ToolError::InvalidInput`.
8. `rejects_unknown_status` — `{"status": "doing"}` →
   `ToolError::InvalidInput` (caught by serde enum deserialization).
9. `output_header_counts_per_status` — write 4-status mix; assert
   header text matches counts.
10. `cancellation_during_write_is_a_noop` — write while
    `cx.cancel.cancel()` fires; either the write completes or the
    pre-write state is preserved; never a half-written list.

Plus an integration test in `caliban-agent-core`'s session module:

11. `persisted_session_roundtrips_todos` — write three todos, serialize
    to JSON, deserialize, assert equality.

Plus a system-prompt rendering test in `caliban-cli`:

12. `system_prompt_appends_todo_block_when_non_empty` — non-empty list
    → prompt contains `--- Current todos ---` and one line per todo.
13. `system_prompt_omits_todo_block_when_empty` — empty list → prompt
    matches pre-todo output.

Target ~13 new tests.

## Risks

- **Tool-context expansion creep.** Once `TodoWriteTool` owns an
  `Arc<Mutex<Session>>`, other tools may want one too. Mitigation: keep
  the `Session` API minimal — `todos`, `model`, `cwd`. Anything else
  goes through the existing `Hooks` layer.
- **System-prompt growth.** 100 todos × ~80 chars ≈ 8 KB of system
  prompt overhead. At Anthropic's prompt-cache pricing this is small
  but non-zero. Mitigation: the 100-item cap; if real use needs more,
  switch to a separate cached prefix.
- **Model confusion when the list contradicts the transcript.** If the
  model marks a todo `completed` but the transcript shows it wasn't,
  there's no detection mechanism. Acceptable: the model is responsible
  for keeping its own list honest, same as Claude Code.
- **`PersistedSession` schema churn.** Loading an older session must
  not break. Mitigation: `#[serde(default)]` on the new field;
  existing sessions deserialize cleanly with an empty `todos` vec.
- **Compactor interaction.** If a future compactor drops the
  `TodoWrite` tool's own tool-result blocks from history, the model
  may lose track of what it wrote. Mitigation: the system-prompt
  surface re-emits the full list every turn, so the model has a
  ground-truth view independent of the transcript.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all -- --check`
  clean.
- `cargo test --workspace` passes — adds ≥ 13 new tests across
  `caliban-tools-builtin`, `caliban-agent-core`, and `caliban-cli`.
- `TodoWriteTool` is re-exported from `caliban_tools_builtin` and
  registered in the `caliban` binary's default tool registry.
- `Session` / `Todo` / `TodoStatus` types are public from
  `caliban-agent-core::session`.
- `PersistedSession` JSON gains a `"todos": [...]` field; older
  sessions on disk continue to load.
- System-prompt builder appends the todo block when the list is
  non-empty.
- README's tool list updated with one sentence about `TodoWrite`.
- **No ADR is required.** This is a tool addition + a small extension
  to `Session`; both are within the existing architectural envelope.

## Open questions

- **Where do `Session` / `Todo` live?** Sketched as
  `caliban-agent-core::session`. If `agent-core` would rather not own
  any session state, lift it to a new `caliban-session` crate. Decide
  during implementation.
- **Hook visibility.** Should `before_tool` see todo writes the same
  way it sees other tool calls? Default: yes (no special case). An
  operator who wants to lock down todos can use the same hook
  surface used to lock down `Bash`.
