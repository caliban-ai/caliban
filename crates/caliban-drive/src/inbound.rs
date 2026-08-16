//! Transport-agnostic inbound frames delivered to a running [`crate::DriveSession`].

use serde::{Deserialize, Serialize};

/// A message sent *into* a running drive session.
///
/// This is the core-owned counterpart to the caliban binary's private
/// `AttachInbound`: living in the drive crate lets every adapter (MCP-server,
/// ACP, HTTP serve) share one inbound vocabulary instead of each re-deriving
/// its own. Outbound is the `TurnEvent` stream (see [`crate::DriveSession::subscribe`]);
/// the two directions never share a frame type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DriveInbound {
    /// Inject a follow-up user message, resuming a run that is awaiting input.
    UserMessage {
        /// The user's message text.
        text: String,
    },
    /// Signal that no further input will be sent; the run ends at its next
    /// end-of-turn boundary.
    EndInput,
}

#[cfg(test)]
mod tests {
    use super::DriveInbound;

    #[test]
    fn serde_tag_shape() {
        let v = serde_json::to_value(DriveInbound::UserMessage { text: "hi".into() }).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "type": "user_message", "text": "hi" })
        );

        let back: DriveInbound =
            serde_json::from_value(serde_json::json!({ "type": "end_input" })).unwrap();
        assert_eq!(back, DriveInbound::EndInput);
    }
}
