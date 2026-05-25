//! Symlink helper: link a fixed list of dirs from the parent worktree
//! into the new worktree.

use std::path::{Path, PathBuf};

use crate::manager::WorktreeError;

/// For each `entry` in `entries`, create a symlink at
/// `worktree_root/<entry>` that points at `parent_root/<entry>`. The
/// parent path must already exist (otherwise we'd be silently creating a
/// dangling link). If the destination already exists inside the new
/// worktree, the call errors so we don't silently overwrite real files.
pub(crate) fn link_all(
    parent_root: &Path,
    worktree_root: &Path,
    entries: &[PathBuf],
) -> Result<(), WorktreeError> {
    for entry in entries {
        if entry.is_absolute() {
            return Err(WorktreeError::BrokenLink {
                path: entry.clone(),
                reason: "symlink_directories entries must be relative to the parent repo root"
                    .into(),
            });
        }
        let src = parent_root.join(entry);
        if !src.exists() {
            return Err(WorktreeError::BrokenLink {
                path: src.clone(),
                reason: "parent path does not exist".into(),
            });
        }
        let dst = worktree_root.join(entry);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WorktreeError::Symlink {
                src: src.clone(),
                dst: dst.clone(),
                source: e,
            })?;
        }
        if dst.exists() {
            return Err(WorktreeError::BrokenLink {
                path: dst.clone(),
                reason: "destination already exists inside worktree".into(),
            });
        }
        symlink_dir(&src, &dst)?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> Result<(), WorktreeError> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| WorktreeError::Symlink {
        src: src.to_path_buf(),
        dst: dst.to_path_buf(),
        source: e,
    })
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> Result<(), WorktreeError> {
    std::os::windows::fs::symlink_dir(src, dst).map_err(|e| WorktreeError::Symlink {
        src: src.to_path_buf(),
        dst: dst.to_path_buf(),
        source: e,
    })
}
