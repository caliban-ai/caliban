//! Sparse-checkout writer.
//!
//! libgit2's `set_sparse_checkout_patterns` API isn't part of the stable
//! `git2` crate surface, so we manage `.git/info/sparse-checkout`
//! manually. The file's format is one pattern per line; presence + the
//! `core.sparseCheckout=true` config knob enables sparse checkout.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use crate::manager::WorktreeError;

/// Write the given patterns to the worktree's `.git/info/sparse-checkout`
/// file. The `git_dir` argument is the worktree's git directory (the
/// "linked worktree dir", typically `<repo>/.git/worktrees/<name>/`).
pub(crate) fn write_patterns(git_dir: &Path, patterns: &[String]) -> Result<(), WorktreeError> {
    if patterns.is_empty() {
        return Ok(());
    }
    let info_dir = git_dir.join("info");
    fs::create_dir_all(&info_dir).map_err(|e| WorktreeError::Sparse {
        path: info_dir.clone(),
        source: e,
    })?;
    let path = info_dir.join("sparse-checkout");
    let mut f = fs::File::create(&path).map_err(|e| WorktreeError::Sparse {
        path: path.clone(),
        source: e,
    })?;
    for p in patterns {
        writeln!(f, "{p}").map_err(|e| WorktreeError::Sparse {
            path: path.clone(),
            source: e,
        })?;
    }
    Ok(())
}
