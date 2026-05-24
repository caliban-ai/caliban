//! Ancestor-walk-up file discovery.
//!
//! Shared utility for finding a configuration / metadata file (e.g.
//! `CLAUDE.md`, `caliban.toml`) by walking up the directory tree from a
//! starting point until a git root or the user's `$HOME` is hit.
//!
//! Caliban's memory tier (ADR 0018) and the model router v2 (`caliban.toml`
//! discovery, ADR 0038) both consume this function.

use std::path::{Path, PathBuf};

/// Walk up the directory tree starting at `start`, returning the first
/// ancestor that contains a file named `filename`. The walk stops at any
/// `.git` directory (treated as a git root) or at `$HOME`, whichever comes
/// first.
///
/// Returns `Some(path-to-file)` if found, `None` if no candidate was hit
/// before the stop boundary.
#[must_use]
pub fn walk_up_for_file(start: &Path, filename: &str) -> Option<PathBuf> {
    let home = dirs::home_dir();
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
        // Stop at git root (when .git exists, this dir IS the root).
        // Check after the candidate, so a caliban.toml at the git root is
        // still discoverable.
        if dir.join(".git").exists() {
            return None;
        }
        // Stop at HOME.
        if let Some(h) = home.as_deref()
            && dir == h
        {
            return None;
        }
        current = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn finds_file_in_starting_dir() {
        let tmp = tempdir().unwrap();
        let f = tmp.path().join("caliban.toml");
        fs::write(&f, "[router]\ndefault_purpose = \"main_loop\"\n").unwrap();
        let found = walk_up_for_file(tmp.path(), "caliban.toml").unwrap();
        // tempdir gives a canonicalized path on macOS; compare via canonicalize.
        assert_eq!(found.canonicalize().unwrap(), f.canonicalize().unwrap());
    }

    #[test]
    fn walks_up_to_ancestor_containing_file() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        let f = tmp.path().join("caliban.toml");
        fs::write(&f, "[router]\ndefault_purpose = \"main_loop\"\n").unwrap();
        // Need a .git at tmp root so the walk terminates there.
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let found = walk_up_for_file(&nested, "caliban.toml").unwrap();
        assert_eq!(found.canonicalize().unwrap(), f.canonicalize().unwrap());
    }

    #[test]
    fn returns_none_when_absent_within_git_root() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("a");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let found = walk_up_for_file(&nested, "caliban.toml");
        assert!(found.is_none());
    }

    #[test]
    fn stops_at_git_root_when_no_file() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("sub");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        // file is OUTSIDE the git root — should not be discovered.
        let outside = tmp.path().parent().unwrap().join("caliban.toml.outside");
        let _ = fs::write(&outside, "x");
        let found = walk_up_for_file(&nested, "caliban.toml");
        assert!(found.is_none());
    }
}
