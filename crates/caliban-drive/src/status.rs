//! Unified run-lifecycle status for a [`crate::DriveSession`].

use serde::{Deserialize, Serialize};

/// Lifecycle state of a driven run, independent of any transport.
///
/// This is the single status representation the drive surfaces (MCP-server,
/// ACP, HTTP serve) report. It unifies what today is split between
/// `caliban_supervisor::proto::AgentStatus` (the daemon's view) and
/// `caliban_agent_core::StopCondition` (the loop's terminal reason).
///
/// Progression: `Starting` → `Running` → (`AwaitingInput` ⇄ `Running`)\* →
/// `Done` | `Failed`. A run that carries no input source never enters
/// `AwaitingInput` — it goes `Running` → `Done`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DriveStatus {
    /// The session has been created but the run has not begun streaming yet.
    Starting,
    /// The run is actively producing turn events.
    Running,
    /// The run reached the end-of-turn boundary and is blocked awaiting input
    /// (a follow-up user message or an end-of-input signal). Only reachable for
    /// interactive sessions.
    AwaitingInput,
    /// The run finished on its own terms (the event stream completed).
    Done,
    /// The run stopped because the underlying event stream yielded an error.
    Failed {
        /// Human-readable description of the failure.
        error: String,
    },
}

impl DriveStatus {
    /// Whether this is a terminal state — no further transitions will occur.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::DriveStatus;

    #[test]
    fn terminal_classification() {
        assert!(!DriveStatus::Starting.is_terminal());
        assert!(!DriveStatus::Running.is_terminal());
        assert!(!DriveStatus::AwaitingInput.is_terminal());
        assert!(DriveStatus::Done.is_terminal());
        assert!(
            DriveStatus::Failed {
                error: "boom".into()
            }
            .is_terminal()
        );
    }

    #[test]
    fn serde_tag_shape() {
        let v = serde_json::to_value(DriveStatus::AwaitingInput).unwrap();
        assert_eq!(v, serde_json::json!({ "state": "awaiting_input" }));

        let v = serde_json::to_value(DriveStatus::Failed {
            error: "nope".into(),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({ "state": "failed", "error": "nope" }));

        let back: DriveStatus =
            serde_json::from_value(serde_json::json!({ "state": "done" })).unwrap();
        assert_eq!(back, DriveStatus::Done);
    }
}
