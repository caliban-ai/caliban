//! Shared todo-list types for the `TodoWrite` tool family.
//!
//! Today there's one consumer: `TodoWriteTool` in `caliban-tools-builtin`. The
//! `caliban` binary creates a [`SharedTodos`] handle at startup, hands a clone
//! to the tool registry, and reads from it when building the per-turn system
//! prompt. `PersistedSession` (in `caliban-sessions`) serializes the snapshot
//! to disk.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Workflow status of a single todo item.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not yet started.
    Pending,
    /// Currently being worked on.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Abandoned before completion.
    Cancelled,
}

/// One entry in the model's structured task list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Todo {
    /// Stable identifier within a single `TodoWrite` payload (≤ 64 chars).
    pub id: String,
    /// Single-line task description (≤ 500 chars; newlines collapsed to spaces).
    pub content: String,
    /// Current status.
    pub status: TodoStatus,
}

/// Shared, mutex-guarded handle to the canonical todo list.
///
/// Cheap to clone (it's just an `Arc`). The mutex is held only briefly: tool
/// invocations replace the entire vec, system-prompt rebuilds snapshot it.
pub type SharedTodos = Arc<Mutex<Vec<Todo>>>;

/// Construct a new empty [`SharedTodos`] handle.
#[must_use]
pub fn new_shared_todos() -> SharedTodos {
    Arc::new(Mutex::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_roundtrips_through_serde_json() {
        let original = Todo {
            id: "1".into(),
            content: "do the thing".into(),
            status: TodoStatus::InProgress,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"in_progress\""));
        let parsed: Todo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn status_uses_snake_case() {
        for (status, expected) in [
            (TodoStatus::Pending, "\"pending\""),
            (TodoStatus::InProgress, "\"in_progress\""),
            (TodoStatus::Completed, "\"completed\""),
            (TodoStatus::Cancelled, "\"cancelled\""),
        ] {
            let s = serde_json::to_string(&status).unwrap();
            assert_eq!(s, expected, "status {status:?}");
        }
    }

    #[test]
    fn shared_todos_can_be_shared_across_clones() {
        let a = new_shared_todos();
        let b = Arc::clone(&a);
        b.lock().unwrap().push(Todo {
            id: "x".into(),
            content: "shared".into(),
            status: TodoStatus::Pending,
        });
        assert_eq!(a.lock().unwrap().len(), 1);
    }
}
