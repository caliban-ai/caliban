//! `PersistedSession` — a saveable conversation.

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
