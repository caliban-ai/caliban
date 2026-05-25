//! Workspace-path sanitization for the per-workspace auto-memory directory.
//!
//! Moved to [`caliban_common::paths::sanitize_cwd_for_path`] as part of the
//! cleanup sprint (PR-T1-A). This thin alias is preserved so existing call
//! sites keep compiling.

use std::path::Path;

/// Build a directory-safe slug from an absolute workspace path. See
/// [`caliban_common::paths::sanitize_cwd_for_path`] for the canonical
/// definition.
#[deprecated(
    since = "0.0.0",
    note = "use `caliban_common::paths::sanitize_cwd_for_path` instead"
)]
#[must_use]
pub fn sanitize_workspace(p: &Path) -> String {
    caliban_common::paths::sanitize_cwd_for_path(p)
}
