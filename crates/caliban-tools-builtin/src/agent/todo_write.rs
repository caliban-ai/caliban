//! `TodoWrite` tool — replaces the shared todo list with the model's new payload.
//!
//! See `docs/superpowers/specs/2026-05-23-todo-write-design.md`.

use std::collections::HashSet;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{SharedTodos, Todo, TodoStatus, Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_TODOS: usize = 100;
const MAX_CONTENT_CHARS: usize = 500;
const MAX_ID_CHARS: usize = 64;

#[derive(Debug, Deserialize)]
struct TodoInput {
    id: String,
    content: String,
    status: TodoStatus,
}

#[derive(Debug, Deserialize)]
struct TodoWriteInput {
    todos: Vec<TodoInput>,
}

/// Built-in `TodoWrite` tool.
///
/// Each instance owns a clone of the canonical [`SharedTodos`] handle and
/// replaces its contents on each invocation. The model uses this to maintain
/// a structured task list across a session; the binary re-emits the list into
/// the system prompt at the start of every user-driven turn.
pub struct TodoWriteTool {
    handle: SharedTodos,
    schema: OnceLock<Value>,
}

impl std::fmt::Debug for TodoWriteTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodoWriteTool").finish_non_exhaustive()
    }
}

impl TodoWriteTool {
    /// Build a [`TodoWriteTool`] from a shared todo-list handle.
    #[must_use]
    pub fn new(handle: SharedTodos) -> Self {
        Self {
            handle,
            schema: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "TodoWrite"
    }

    fn description(&self) -> &'static str {
        "Replace the current session todo list. Pass the entire array; reordering means \
         reordering the array; deletion means omitting items. Status is one of \
         pending|in_progress|completed|cancelled. The list is surfaced back to you in \
         every user-driven turn's system prompt."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id":      { "type": "string", "description": "Stable id within the payload (<= 64 chars)" },
                                "content": { "type": "string", "description": "Single-line task description (<= 500 chars)" },
                                "status":  { "enum": ["pending", "in_progress", "completed", "cancelled"] }
                            },
                            "required": ["id", "content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            })
        })
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: TodoWriteInput = crate::parse_input(input)?;

        if parsed.todos.len() > MAX_TODOS {
            return Err(ToolError::invalid_input(format!(
                "too many todos: {} (max {MAX_TODOS})",
                parsed.todos.len()
            )));
        }

        let mut seen: HashSet<&str> = HashSet::new();
        for t in &parsed.todos {
            if t.id.chars().count() > MAX_ID_CHARS {
                return Err(ToolError::invalid_input(format!(
                    "todo id too long (>{MAX_ID_CHARS} chars): {}",
                    t.id
                )));
            }
            if t.content.chars().count() > MAX_CONTENT_CHARS {
                return Err(ToolError::invalid_input(format!(
                    "todo content too long (>{MAX_CONTENT_CHARS} chars) for id={}",
                    t.id
                )));
            }
            if !seen.insert(t.id.as_str()) {
                return Err(ToolError::invalid_input(format!(
                    "duplicate todo id in payload: {}",
                    t.id
                )));
            }
        }

        let todos: Vec<Todo> = parsed
            .todos
            .into_iter()
            .map(|t| Todo {
                id: t.id,
                content: collapse_newlines(&t.content),
                status: t.status,
            })
            .collect();

        let header = format_header(&todos);

        // Snapshot the previous list to compute status transitions.
        let prev: Vec<Todo> = {
            let guard = self.handle.lock().map_err(|e| {
                ToolError::execution(std::io::Error::other(format!("lock poisoned: {e}")))
            })?;
            guard.clone()
        };

        {
            let mut guard = self.handle.lock().map_err(|e| {
                ToolError::execution(std::io::Error::other(format!("lock poisoned: {e}")))
            })?;
            guard.clone_from(&todos);
        }

        // Fire TaskCreated / TaskCompleted on transitions (best-effort).
        if let Some(hooks) = cx.hooks.as_ref() {
            fire_task_hooks(&prev, &todos, hooks.as_ref()).await;
        }

        Ok(vec![ContentBlock::Text(TextBlock {
            text: header,
            cache_control: None,
        })])
    }
}

fn status_str(s: TodoStatus) -> &'static str {
    match s {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in_progress",
        TodoStatus::Completed => "completed",
        TodoStatus::Cancelled => "cancelled",
    }
}

fn is_terminal(s: TodoStatus) -> bool {
    matches!(s, TodoStatus::Completed | TodoStatus::Cancelled)
}

async fn fire_task_hooks(
    prev: &[Todo],
    new: &[Todo],
    hooks: &(dyn caliban_agent_core::Hooks + Send + Sync),
) {
    use std::collections::HashMap;
    let prev_map: HashMap<&str, TodoStatus> =
        prev.iter().map(|t| (t.id.as_str(), t.status)).collect();
    for t in new {
        let was = prev_map.get(t.id.as_str()).copied();
        // TaskCreated fires when a new task appears or transitions from
        // pending to in_progress.
        let appearing = was.is_none();
        let activated =
            matches!(was, Some(TodoStatus::Pending)) && matches!(t.status, TodoStatus::InProgress);
        if appearing || activated {
            let task_ctx = caliban_agent_core::TaskCtx {
                task_id: &t.id,
                content: &t.content,
                status: status_str(t.status),
            };
            if let Err(e) = hooks.task_created(&task_ctx).await {
                tracing::warn!(error = %e, "task_created hook error (non-fatal)");
            }
        }
        // TaskCompleted fires when the status transitions to a terminal
        // state (completed / cancelled) from a non-terminal state.
        let was_non_terminal = was.is_none_or(|s| !is_terminal(s));
        if was_non_terminal && is_terminal(t.status) {
            let task_ctx = caliban_agent_core::TaskCtx {
                task_id: &t.id,
                content: &t.content,
                status: status_str(t.status),
            };
            let outcome = caliban_agent_core::TaskOutcome {
                terminal_status: status_str(t.status).to_string(),
            };
            if let Err(e) = hooks.task_completed(&task_ctx, &outcome).await {
                tracing::warn!(error = %e, "task_completed hook error (non-fatal)");
            }
        }
    }
}

fn collapse_newlines(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

fn format_header(todos: &[Todo]) -> String {
    if todos.is_empty() {
        return "→ TodoWrite: list cleared".to_string();
    }
    let mut pending = 0_usize;
    let mut in_progress = 0_usize;
    let mut completed = 0_usize;
    let mut cancelled = 0_usize;
    for t in todos {
        match t.status {
            TodoStatus::Pending => pending += 1,
            TodoStatus::InProgress => in_progress += 1,
            TodoStatus::Completed => completed += 1,
            TodoStatus::Cancelled => cancelled += 1,
        }
    }
    format!(
        "→ TodoWrite: {} total ({pending} pending, {in_progress} in-progress, {completed} completed, {cancelled} cancelled)",
        todos.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::new_shared_todos;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> ToolContext {
        ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
            hooks: None,
            turn_index: 0,
        }
    }

    fn text_of(blocks: &[ContentBlock]) -> String {
        match &blocks[0] {
            ContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text block"),
        }
    }

    #[tokio::test]
    async fn accepts_empty_list_clears_state() {
        let handle = new_shared_todos();
        handle.lock().unwrap().push(Todo {
            id: "x".into(),
            content: "stale".into(),
            status: TodoStatus::Pending,
        });
        let tool = TodoWriteTool::new(handle.clone());
        let out = tool.invoke(json!({"todos": []}), ctx()).await.unwrap();
        assert_eq!(text_of(&out), "→ TodoWrite: list cleared");
        assert!(handle.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replaces_existing_list_completely() {
        let handle = new_shared_todos();
        handle.lock().unwrap().extend([
            Todo {
                id: "a".into(),
                content: "old1".into(),
                status: TodoStatus::Pending,
            },
            Todo {
                id: "b".into(),
                content: "old2".into(),
                status: TodoStatus::Pending,
            },
        ]);
        let tool = TodoWriteTool::new(handle.clone());
        let payload = json!({
            "todos": [{ "id": "z", "content": "fresh", "status": "in_progress" }]
        });
        tool.invoke(payload, ctx()).await.unwrap();
        let stored = handle.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, "z");
        assert_eq!(stored[0].status, TodoStatus::InProgress);
    }

    #[tokio::test]
    async fn preserves_order_from_input() {
        let handle = new_shared_todos();
        let tool = TodoWriteTool::new(handle.clone());
        let payload = json!({
            "todos": [
                { "id": "3", "content": "third", "status": "pending" },
                { "id": "1", "content": "first", "status": "pending" },
                { "id": "2", "content": "second", "status": "pending" },
            ]
        });
        tool.invoke(payload, ctx()).await.unwrap();
        let ids: Vec<_> = handle
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(ids, vec!["3", "1", "2"]);
    }

    #[tokio::test]
    async fn rejects_duplicate_ids_in_one_payload() {
        let tool = TodoWriteTool::new(new_shared_todos());
        let payload = json!({
            "todos": [
                { "id": "1", "content": "a", "status": "pending" },
                { "id": "1", "content": "b", "status": "pending" },
            ]
        });
        let err = tool.invoke(payload, ctx()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rejects_oversize_list() {
        let tool = TodoWriteTool::new(new_shared_todos());
        let todos: Vec<Value> = (0..=MAX_TODOS)
            .map(|i| json!({ "id": i.to_string(), "content": "x", "status": "pending" }))
            .collect();
        let err = tool
            .invoke(json!({ "todos": todos }), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rejects_oversize_content() {
        let tool = TodoWriteTool::new(new_shared_todos());
        let big = "a".repeat(MAX_CONTENT_CHARS + 1);
        let err = tool
            .invoke(
                json!({ "todos": [{ "id": "1", "content": big, "status": "pending" }] }),
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rejects_oversize_id() {
        let tool = TodoWriteTool::new(new_shared_todos());
        let big = "a".repeat(MAX_ID_CHARS + 1);
        let err = tool
            .invoke(
                json!({ "todos": [{ "id": big, "content": "x", "status": "pending" }] }),
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rejects_unknown_status() {
        let tool = TodoWriteTool::new(new_shared_todos());
        let err = tool
            .invoke(
                json!({ "todos": [{ "id": "1", "content": "x", "status": "doing" }] }),
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn output_header_counts_per_status() {
        let tool = TodoWriteTool::new(new_shared_todos());
        let payload = json!({
            "todos": [
                { "id": "1", "content": "a", "status": "pending" },
                { "id": "2", "content": "b", "status": "in_progress" },
                { "id": "3", "content": "c", "status": "completed" },
                { "id": "4", "content": "d", "status": "completed" },
                { "id": "5", "content": "e", "status": "cancelled" },
            ]
        });
        let out = tool.invoke(payload, ctx()).await.unwrap();
        let text = text_of(&out);
        assert!(
            text.contains("5 total")
                && text.contains("1 pending")
                && text.contains("1 in-progress")
                && text.contains("2 completed")
                && text.contains("1 cancelled"),
            "header mismatch: {text}"
        );
    }

    #[tokio::test]
    async fn newlines_in_content_are_collapsed() {
        let handle = new_shared_todos();
        let tool = TodoWriteTool::new(handle.clone());
        let payload = json!({
            "todos": [{ "id": "1", "content": "line one\nline two\rline three", "status": "pending" }]
        });
        tool.invoke(payload, ctx()).await.unwrap();
        let stored = handle.lock().unwrap();
        assert_eq!(stored[0].content, "line one line two line three");
    }
}
