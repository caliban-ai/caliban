//! Helpers for the `Tool::parallel_conflict_key` override (ADR 0016 Revised).

use std::path::{Path, PathBuf};

/// Canonicalize a file path for use as a `parallel_conflict_key`.
///
/// Tries `std::fs::canonicalize` first so two paths that differ only in
/// symlink chains, `.`/`..` components, or relative-vs-absolute form collapse
/// to one identity. When canonicalize fails (e.g., `Write` creating a new
/// file), falls back to canonicalizing the parent directory and joining the
/// file name. When both fall through, returns the raw input string — which is
/// at worst over-conservative (string-equal paths still collide).
#[must_use]
pub(crate) fn canonical_key(path: &str) -> String {
    let p = Path::new(path);
    if let Ok(c) = p.canonicalize() {
        return c.display().to_string();
    }
    if let (Some(parent), Some(file)) = (p.parent(), p.file_name())
        && let Ok(parent_c) = parent.canonicalize()
    {
        return parent_c.join(file).display().to_string();
    }
    // Last resort: absolute-ize via cwd-join so two different relative paths
    // from different working directories don't accidentally collide.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if p.is_absolute() {
        p.display().to_string()
    } else {
        cwd.join(p).display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_falls_back_to_absolutized_for_nonexistent_path() {
        // Random nonexistent path under /tmp. Parent (/tmp) exists, so the
        // parent-canonicalize-plus-filename branch fires.
        let key = canonical_key("/tmp/does-not-exist-xyz-12345");
        assert!(key.ends_with("/does-not-exist-xyz-12345"));
    }

    #[test]
    fn canonical_key_collapses_dot_components() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("file.txt");
        std::fs::write(&target, "x").unwrap();
        let direct = canonical_key(target.to_str().unwrap());
        let with_dot = canonical_key(&format!("{}/./file.txt", dir.path().display()));
        assert_eq!(direct, with_dot);
    }
}
