//! Serializable parent permission policy, inherited by background
//! sub-agents when `inherit_hooks=true` (ADR 0037 / #84).
//!
//! `caliban-supervisor` (home of `SpawnSpec`) does not depend on
//! `caliban-agent-core`, so this config crosses the process boundary as an
//! opaque JSON string in `SpawnSpec.inherited_hooks_config`. The parent
//! serializes it; the worker deserializes and rebuilds the chain. Only the
//! config-expressible portion crosses — closure hooks are dropped (ADR 0037).

use caliban_agent_core::{PermissionMode, Rule};
use serde::{Deserialize, Serialize};

/// The config-expressible slice of a parent's permission chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InheritableHookConfig {
    /// Resolved permission rules (CLI + settings + defaults + MCP), in
    /// evaluation order (first match wins).
    pub rules: Vec<Rule>,
    /// Parent permission mode (default / acceptEdits / plan / auto / …).
    pub mode: PermissionMode,
    /// Whether decision audit logging is enabled.
    pub audit: bool,
}

impl InheritableHookConfig {
    /// Serialize to the opaque JSON carried in `SpawnSpec`.
    pub(crate) fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    /// Parse the opaque JSON from `SpawnSpec`. Returns `None` on malformed
    /// input (the worker then falls back to its default gate).
    pub(crate) fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::{Action, PermissionMode};

    #[test]
    fn round_trips_through_json() {
        let cfg = InheritableHookConfig {
            rules: vec![
                Rule {
                    tool: "Bash".into(),
                    action: Action::Deny,
                    comment: None,
                    reason: None,
                    expires_at: None,
                },
                Rule {
                    tool: "Read".into(),
                    action: Action::Allow,
                    comment: None,
                    reason: None,
                    expires_at: None,
                },
            ],
            mode: PermissionMode::AcceptEdits,
            audit: true,
        };
        let json = cfg.to_json().expect("serialize");
        let back = InheritableHookConfig::from_json(&json).expect("deserialize");
        assert_eq!(back.rules.len(), 2);
        assert_eq!(back.rules[0].tool, "Bash");
        assert!(matches!(back.rules[0].action, Action::Deny));
        assert_eq!(back.mode, PermissionMode::AcceptEdits);
        assert!(back.audit);
    }

    #[test]
    fn from_json_rejects_garbage() {
        assert!(InheritableHookConfig::from_json("not json").is_none());
    }
}
