//! Permission modes — `default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/
//! `bypassPermissions` cycled via Shift+Tab in the TUI (ADR 0029).
//!
//! See `docs/superpowers/specs/2026-05-24-permission-modes-design.md`.

use std::sync::Arc;

use arc_swap::ArcSwap;

// `PermissionMode` is a plain config data type; it now lives in the
// `caliban-config-types` leaf crate (epic #539 / ADR 0061). Re-exported here so
// `caliban_agent_core::permission_mode::PermissionMode` (and the crate-root
// re-export in `lib.rs`) keep resolving unchanged. The shared `ArcSwap` handle
// and the startup-gating logic below stay in this crate.
pub use caliban_config_types::permission_mode::PermissionMode;

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
/// 2. `permissions.default_mode` from the settings layer.
/// 3. Built-in [`PermissionMode::Default`].
///
/// The `CALIBAN_DEFAULT_PERMISSION_MODE` environment variable is **not** read
/// here: the settings env layer folds it into `permissions.default_mode` at load
/// time (env > file), so it participates as part of `settings_default_mode` while
/// keeping the CLI flag highest-precedence (#701; env registry #538). This keeps
/// the original CLI > env > file ordering with a single env-read site.
///
/// Pass `bypass_latch = true` when `--allow-dangerously-skip-permissions`
/// is on the command line. Without the latch, asking for
/// [`PermissionMode::BypassPermissions`] is a hard error.
///
/// # Errors
/// Returns an error string when:
/// - The CLI value or the resolved settings mode doesn't parse as a known mode.
/// - The resolved mode is [`PermissionMode::BypassPermissions`] without
///   `bypass_latch` set.
pub fn resolve_startup_mode(
    cli: Option<&str>,
    settings_default_mode: Option<&str>,
    bypass_latch: bool,
) -> Result<PermissionMode, String> {
    let mode = if let Some(s) = cli {
        PermissionMode::parse(s)
            .map_err(|bad| format!("--permission-mode: unknown mode '{bad}'"))?
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
