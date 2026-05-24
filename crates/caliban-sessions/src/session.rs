//! `PersistedSession` — a saveable conversation.

use caliban_agent_core::Todo;
use caliban_provider::{Message, Role, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A conversation session, suitable for persisting to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    /// The unique name of this session.
    pub name: String,
    /// When the session was first created.
    pub created_at: DateTime<Utc>,
    /// When the session was last modified.
    pub updated_at: DateTime<Utc>,
    /// The provider used for this session (e.g. `"anthropic"`).
    pub provider: String,
    /// The model used for this session.
    pub model: String,
    /// The conversation history.
    pub messages: Vec<Message>,
    /// Accumulated token usage across all turns.
    pub total_usage: Usage,
    /// Structured task list maintained by the model via `TodoWrite`.
    /// Pre-todo sessions on disk deserialize with an empty vec.
    #[serde(default)]
    pub todos: Vec<Todo>,
    /// Plan-mode flag — when `true`, the dispatcher rejects mutating tools.
    /// Pre-plan-mode sessions on disk deserialize with `false`.
    #[serde(default)]
    pub plan_mode: bool,
}

impl PersistedSession {
    /// Construct a new empty session.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            created_at: now,
            updated_at: now,
            provider: provider.into(),
            model: model.into(),
            messages: Vec::new(),
            total_usage: Usage::default(),
            todos: Vec::new(),
            plan_mode: false,
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
        u32::try_from(
            self.messages
                .iter()
                .filter(|m| m.role == Role::Assistant)
                .count(),
        )
        .unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::TodoStatus;

    #[test]
    fn persisted_session_roundtrips_todos() {
        let mut s = PersistedSession::new("t", "anthropic", "claude-3-5-sonnet");
        s.todos = vec![
            Todo {
                id: "1".into(),
                content: "first".into(),
                status: TodoStatus::Pending,
            },
            Todo {
                id: "2".into(),
                content: "second".into(),
                status: TodoStatus::InProgress,
            },
        ];
        let json = serde_json::to_string(&s).unwrap();
        let parsed: PersistedSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.todos, s.todos);
    }

    #[test]
    fn legacy_session_without_todos_loads_with_empty_vec() {
        // Construct a JSON value missing the `todos` field — emulates an
        // on-disk session from before this change.
        let now = Utc::now();
        let json = serde_json::json!({
            "name": "legacy",
            "created_at": now,
            "updated_at": now,
            "provider": "anthropic",
            "model": "claude-3-5-sonnet",
            "messages": [],
            "total_usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_input_tokens": null,
                "cache_read_input_tokens": null
            }
        });
        let parsed: PersistedSession = serde_json::from_value(json).unwrap();
        assert!(parsed.todos.is_empty());
    }
}
