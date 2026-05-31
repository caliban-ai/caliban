//! Permission modes — `default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/
//! `bypassPermissions` cycled via Shift+Tab in the TUI (ADR 0029).
//!
//! See `docs/superpowers/specs/2026-05-24-permission-modes-design.md`.

use std::sync::Arc;

use arc_swap::ArcSwap;
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
    /// Existing plan-mode allowlist (read-only tools); legacy
    /// [`crate::SharedPlanMode`] follows.
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

// ---------------------------------------------------------------------------
// SharedPermissionMode — lock-free reads via `ArcSwap`
// ---------------------------------------------------------------------------

/// Shared, lock-free-readable handle to the current [`PermissionMode`].
/// Cheap to clone. `Shift+Tab` in the TUI calls
/// [`SharedPermissionMode::store`] without taking any locks.
#[derive(Debug, Clone)]
pub struct SharedPermissionMode {
    inner: Arc<ArcSwap<PermissionMode>>,
}

impl SharedPermissionMode {
    /// Construct a handle initialized to `mode`.
    #[must_use]
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(mode)),
        }
    }

    /// Read the current mode.
    #[must_use]
    pub fn load(&self) -> PermissionMode {
        **self.inner.load()
    }

    /// Replace the current mode.
    pub fn store(&self, mode: PermissionMode) {
        self.inner.store(Arc::new(mode));
    }
}

impl Default for SharedPermissionMode {
    fn default() -> Self {
        Self::new(PermissionMode::Default)
    }
}

// ---------------------------------------------------------------------------
// Startup gating
// ---------------------------------------------------------------------------

/// Resolve the initial [`PermissionMode`] at startup using the documented
/// precedence:
///
/// 1. `--permission-mode <mode>` CLI value (when provided).
/// 2. `CALIBAN_DEFAULT_PERMISSION_MODE` environment variable.
/// 3. `permissions.default_mode` from the settings file.
/// 4. Built-in [`PermissionMode::Default`].
///
/// Pass `bypass_latch = true` when `--allow-dangerously-skip-permissions`
/// is on the command line. Without the latch, asking for
/// [`PermissionMode::BypassPermissions`] is a hard error.
///
/// # Errors
/// Returns an error string when:
/// - The CLI value or env var doesn't parse as a known mode.
/// - The resolved mode is [`PermissionMode::BypassPermissions`] without
///   `bypass_latch` set.
pub fn resolve_startup_mode(
    cli: Option<&str>,
    env_var: Option<&str>,
    settings_default_mode: Option<&str>,
    bypass_latch: bool,
) -> Result<PermissionMode, String> {
    let mode = if let Some(s) = cli {
        PermissionMode::parse(s)
            .map_err(|bad| format!("--permission-mode: unknown mode '{bad}'"))?
    } else if let Some(s) = env_var {
        PermissionMode::parse(s)
            .map_err(|bad| format!("CALIBAN_DEFAULT_PERMISSION_MODE: unknown mode '{bad}'"))?
    } else if let Some(s) = settings_default_mode {
        PermissionMode::parse(s)
            .map_err(|bad| format!("permissions.default_mode: unknown mode '{bad}'"))?
    } else {
        PermissionMode::Default
    };
    if mode == PermissionMode::BypassPermissions && !bypass_latch {
        return Err("bypassPermissions requires --allow-dangerously-skip-permissions".into());
    }
    Ok(mode)
}

// ---------------------------------------------------------------------------
// Tool classification helpers
// ---------------------------------------------------------------------------

/// File-edit tools that `acceptEdits` mode auto-allows.
pub const FILE_EDIT_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// Returns `true` when `tool_name` is one of [`FILE_EDIT_TOOLS`].
#[must_use]
pub fn is_file_edit_tool(tool_name: &str) -> bool {
    FILE_EDIT_TOOLS.contains(&tool_name)
}
