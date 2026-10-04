//! Permission mode data type — `default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/
//! `bypassPermissions` (ADR 0029).
//!
//! Only the plain `PermissionMode` enum lives here (the shared `ArcSwap` handle
//! and the startup-gating logic stay in `caliban-agent-core`). See
//! `docs/superpowers/specs/2026-05-24-permission-modes-design.md`.

use serde::{Deserialize, Serialize};

/// One of the six permission modes operators can cycle through.
///
/// The order here matches the `Shift+Tab` cycle:
/// `default → acceptEdits → plan → auto → dontAsk → bypassPermissions → default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Rules apply unchanged; `Ask` routes to the modal.
    #[default]
    Default,
    /// `Write`/`Edit`/`MultiEdit`/`NotebookEdit` auto-allow; other tools
    /// honor rules.
    AcceptEdits,
    /// Existing plan-mode allowlist (read-only tools); the legacy shared
    /// plan-mode handle follows.
    Plan,
    /// Classifier-driven: a fast model labels each tool call as
    /// `allow`/`soft_deny`/`hard_deny`.
    Auto,
    /// Every `Ask` becomes `Allow` (CI-friendly, but rules still apply).
    DontAsk,
    /// Kill switch; rules ignored. Requires the
    /// `--allow-dangerously-skip-permissions` confirmation flag.
    BypassPermissions,
}

impl PermissionMode {
    /// Cycle to the next mode. Wraps around at the end.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::AcceptEdits,
            Self::AcceptEdits => Self::Plan,
            Self::Plan => Self::Auto,
            Self::Auto => Self::DontAsk,
            Self::DontAsk => Self::BypassPermissions,
            Self::BypassPermissions => Self::Default,
        }
    }

    /// Cycle to the previous mode. Wraps around.
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Default => Self::BypassPermissions,
            Self::AcceptEdits => Self::Default,
            Self::Plan => Self::AcceptEdits,
            Self::Auto => Self::Plan,
            Self::DontAsk => Self::Auto,
            Self::BypassPermissions => Self::DontAsk,
        }
    }

    /// Short status-bar chip text for this mode, or empty string for
    /// [`Self::Default`].
    #[must_use]
    pub fn chip(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::AcceptEdits => "\u{270e} accept edits",
            Self::Plan => "\u{1f4cb} plan",
            Self::Auto => "\u{1f916} auto",
            Self::DontAsk => "\u{23ed} don't ask",
            Self::BypassPermissions => "\u{26a0} bypass",
        }
    }

    /// Parse a camelCase identifier matching the CLI / settings.json
    /// representation.
    ///
    /// # Errors
    /// Returns the input string when it doesn't match any known mode.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "default" => Ok(Self::Default),
            "acceptEdits" => Ok(Self::AcceptEdits),
            "plan" => Ok(Self::Plan),
            "auto" => Ok(Self::Auto),
            "dontAsk" => Ok(Self::DontAsk),
            "bypassPermissions" => Ok(Self::BypassPermissions),
            other => Err(other.into()),
        }
    }

    /// Inverse of [`Self::parse`].
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::DontAsk => "dontAsk",
            Self::BypassPermissions => "bypassPermissions",
        }
    }
}

impl std::str::FromStr for PermissionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
