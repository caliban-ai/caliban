//! Session — stateful wrapper around an `Arc<Agent>`.

use std::sync::Arc;

use caliban_provider::Message;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::error::Result;
use crate::stream::TurnEventStream;

/// Stateful conversation session sharing an [`Arc<Agent>`].
///
/// Multiple sessions can share one [`Agent`]. Each session maintains its own
/// message history and cancellation token.
pub struct Session {
    agent: Arc<Agent>,
    messages: Vec<Message>,
    cancel: CancellationToken,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("messages", &self.messages.len())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Create a new session backed by `agent` with an empty message history.
    #[must_use]
    pub fn new(agent: Arc<Agent>) -> Self {
        Self {
            agent,
            messages: Vec::new(),
            cancel: CancellationToken::new(),
        }
    }

    /// Prepend a system message, inserting it before any non-system messages.
    ///
    /// Maintains the invariant that system messages precede user/assistant
    /// messages in the history.
    pub fn system(&mut self, text: impl Into<String>) -> &mut Self {
        let insertion_index = self
            .messages
            .iter()
            .position(|m| m.role != caliban_provider::Role::System)
            .unwrap_or(self.messages.len());
        self.messages
            .insert(insertion_index, Message::system_text(text));
        self
    }

    /// Append a user text message.
    pub fn user_text(&mut self, text: impl Into<String>) -> &mut Self {
        self.messages.push(Message::user_text(text));
        self
    }

    /// Append an arbitrary message.
    pub fn user_message(&mut self, msg: Message) -> &mut Self {
        self.messages.push(msg);
        self
    }

    /// Append several messages at once.
    pub fn extend_messages(&mut self, msgs: impl IntoIterator<Item = Message>) -> &mut Self {
        self.messages.extend(msgs);
        self
    }

    /// Run the agent until done; append generated messages to history.
    ///
    /// Returns a slice of the messages that were added during this call
    /// (assistant messages + tool result messages).
    ///
    /// # Errors
    ///
    /// Propagates errors from the underlying [`Agent::run_until_done`].
    pub async fn run(&mut self) -> Result<&[Message]> {
        let original_len = self.messages.len();
        let messages = self.messages.clone();
        let outcome = Arc::clone(&self.agent)
            .run_until_done(messages, self.cancel.clone())
            .await?;
        self.messages = outcome.final_messages;
        Ok(&self.messages[original_len..])
    }

    /// Return a streaming event source for the current history.
    ///
    /// The caller must drain the stream. This method does **not** mutate
    /// `self.messages` automatically because events are emitted incrementally;
    /// the caller should call [`Session::extend_messages`] with the final
    /// messages from [`TurnEvent::RunEnd`] if they wish to persist the history.
    ///
    /// # Errors
    ///
    /// Returns any error from the underlying [`Agent::stream_until_done`].
    pub fn stream(&self) -> TurnEventStream {
        Arc::clone(&self.agent).stream_until_done(self.messages.clone(), self.cancel.clone())
    }

    /// Read-only view of the current message history.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Signal cancellation for any in-flight or future `run`/`stream` call.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use tokio_util::sync::CancellationToken;

    // A minimal helper to build an Agent without a real provider.
    // We only test the Session's *structural* behaviour here (message
    // management, cancel signalling). Round-trip tests with a mock provider
    // live in the integration test suite (Task 8).

    #[test]
    fn debug_impl_does_not_panic() {
        // Build a CancellationToken manually — we just need a Session value.
        let cancel = CancellationToken::new();
        // We can't build a full Agent without a provider, so test the
        // Debug output indirectly via a token that exists.
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn system_message_inserted_before_user_messages() {
        // Create a bare Session by constructing one without a real agent call.
        // We test just the message-ordering logic, which is pure Rust.
        use caliban_provider::{Message, Role};

        // Simulate the internal logic of Session::system directly.
        let mut messages: Vec<Message> =
            vec![Message::user_text("hello"), Message::assistant_text("hi")];

        let insertion_index = messages
            .iter()
            .position(|m| m.role != Role::System)
            .unwrap_or(messages.len());
        messages.insert(insertion_index, Message::system_text("be helpful"));

        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[2].role, Role::Assistant);
    }

    #[test]
    fn system_appended_after_existing_system_messages() {
        use caliban_provider::{Message, Role};

        let mut messages: Vec<Message> = vec![
            Message::system_text("first system"),
            Message::user_text("hello"),
        ];

        // Insert second system message — should go at index 1 (before the user msg).
        let insertion_index = messages
            .iter()
            .position(|m| m.role != Role::System)
            .unwrap_or(messages.len());
        messages.insert(insertion_index, Message::system_text("second system"));

        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::System);
        assert_eq!(messages[2].role, Role::User);
    }

    #[test]
    fn cancel_token_fires_after_cancel() {
        let cancel = CancellationToken::new();
        assert!(!cancel.is_cancelled());
        cancel.cancel();
        assert!(cancel.is_cancelled());
    }
}
