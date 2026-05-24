# TodoWrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: `superpowers:subagent-driven-development` or `superpowers:executing-plans`.

**Goal:** Add a `TodoWrite` built-in tool that lets the model maintain a structured task list across a session; list is appended to the system prompt at the start of each REPL turn.

**Architecture:** New `caliban-agent-core::session` module owns `Todo` / `TodoStatus` / `SharedTodos = Arc<Mutex<Vec<Todo>>>`. `TodoWriteTool` owns one handle clone; the binary owns another and uses it to rebuild message[0] before each `stream_until_done` call (TUI + non-TUI path). `PersistedSession` gains `todos: Vec<Todo>` with `#[serde(default)]`.

**Spec:** `docs/superpowers/specs/2026-05-23-todo-write-design.md`

---

## File map

| Path | Action |
|---|---|
| `crates/caliban-agent-core/src/session.rs` | create — Todo/TodoStatus/SharedTodos |
| `crates/caliban-agent-core/src/lib.rs` | re-export new module |
| `crates/caliban-tools-builtin/src/todo_write.rs` | create — TodoWriteTool + 10 unit tests |
| `crates/caliban-tools-builtin/src/lib.rs` | pub mod / re-export |
| `crates/caliban-sessions/Cargo.toml` | add `caliban-agent-core` dep |
| `crates/caliban-sessions/src/session.rs` | add `todos: Vec<Todo>` field |
| `caliban/src/system_prompt.rs` | optional `todos: &[Todo]` parameter; append block when non-empty |
| `caliban/src/main.rs` | create handle, register tool, sync session ↔ handle, rebuild message[0] before each run |
| `caliban/src/tui.rs` | rebuild message[0] before each user-driven `stream_until_done` call |
| `README.md` | one sentence in tool list |

---

## Task 1: Todo types in agent-core

- [ ] Create `crates/caliban-agent-core/src/session.rs`:
  - `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)] pub enum TodoStatus { Pending, InProgress, Completed, Cancelled }` with `#[serde(rename_all = "snake_case")]`.
  - `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)] pub struct Todo { pub id: String, pub content: String, pub status: TodoStatus }`.
  - `pub type SharedTodos = std::sync::Arc<std::sync::Mutex<Vec<Todo>>>;`.
  - `pub fn new_shared_todos() -> SharedTodos`.
- [ ] Add `pub mod session;` and `pub use session::{Todo, TodoStatus, SharedTodos, new_shared_todos};` to `lib.rs`.
- [ ] Unit test: roundtrip Todo via serde_json.
- [ ] Verify build: `cargo check -p caliban-agent-core`.
- [ ] Commit.

## Task 2: TodoWriteTool

- [ ] Create `crates/caliban-tools-builtin/src/todo_write.rs`:
  - `pub struct TodoWriteTool { handle: SharedTodos, schema: OnceLock<Value> }`.
  - `pub fn new(handle: SharedTodos) -> Self`.
  - Tool::name = `"TodoWrite"`, description matches spec.
  - JSON schema from spec.
  - `invoke`:
    - Deserialize input → `{todos: Vec<TodoInput>}`.
    - Validate: ≤100 items, content ≤500, id ≤64, no dup ids in payload.
    - On any error → `ToolError::InvalidInput`.
    - Replace handle contents.
    - Build output `→ TodoWrite: N total (P pending, I in-progress, C completed, X cancelled)` or `→ TodoWrite: list cleared` when empty.
    - Return single TextBlock.
- [ ] All 10 unit tests from spec section "Testing".
- [ ] Re-export in `crates/caliban-tools-builtin/src/lib.rs`.
- [ ] Run `cargo test -p caliban-tools-builtin todo_write`.
- [ ] Commit.

## Task 3: Persist todos through PersistedSession

- [ ] `crates/caliban-sessions/Cargo.toml`: add `caliban-agent-core = { path = "../caliban-agent-core" }`.
- [ ] `crates/caliban-sessions/src/session.rs`: add `#[serde(default)] pub todos: Vec<Todo>` field; initialize in `new()`.
- [ ] Add integration test `persisted_session_roundtrips_todos` proving roundtrip and that legacy sessions (no `todos` key in JSON) load with empty vec.
- [ ] Run `cargo test -p caliban-sessions`.
- [ ] Commit.

## Task 4: System-prompt builder reads todos

- [ ] Extend `caliban/src/system_prompt.rs` with a helper `pub(crate) fn append_todo_block(prompt: &str, todos: &[Todo]) -> String`:
  - If todos empty → return prompt as-is.
  - Else append `\n--- Current todos ---\n` plus one line per todo using glyphs `[ ] (id) content` / `[~]` / `[x]` / `[-]`.
- [ ] Two unit tests in `system_prompt.rs`:
  - `system_prompt_appends_todo_block_when_non_empty`
  - `system_prompt_omits_todo_block_when_empty`
- [ ] Commit.

## Task 5: Wire handle into main.rs

- [ ] In `caliban/src/main.rs`:
  - Build `let todos = caliban_agent_core::new_shared_todos();`.
  - If session loaded with non-empty `persisted.todos`, copy into the handle.
  - Pass `Arc::clone(&todos)` to `TodoWriteTool::new(...)` inside `build_registry`.
  - Add helper `fn install_system_prompt(messages: &mut Vec<Message>, base: &Option<String>, todos: &SharedTodos)` that replaces message[0] (or inserts at idx 0 if missing) with `system_text(append_todo_block(base, &snapshot))`.
  - Call `install_system_prompt` right before invoking the non-TUI agent run.
  - After the run, snapshot the handle into `session.todos` before saving.
- [ ] Verify `cargo build -p caliban`.
- [ ] Commit.

## Task 6: TUI integration

- [ ] In `caliban/src/tui.rs`, plumb the `SharedTodos` handle (parameter to `run`).
- [ ] Before each `agent.stream_until_done(...)` call (i.e., per user-driven turn), call `install_system_prompt(&mut session.messages, &system_prompt, &todos)`.
- [ ] After each run, snapshot back into `session.todos`.
- [ ] Verify `cargo build -p caliban`.
- [ ] Commit.

## Task 7: README + final verification

- [ ] Add one sentence in the README's tool list:
  - `**TodoWrite**(todos) — maintain a structured task list across the session; surfaced back into the system prompt each turn.`
- [ ] `cargo fmt --all`.
- [ ] `cargo test --workspace`.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] ci-cloud clippy.
- [ ] Push, open PR, merge after CI passes.
