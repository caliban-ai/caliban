//! Workspace-path sanitization for the per-workspace auto-memory directory.

use std::path::Path;

/// Build a directory-safe slug from an absolute workspace path.
///
/// Rules (see the design spec for examples):
/// 1. Canonicalize via [`std::fs::canonicalize`] (best-effort; fall back to the
///    original path on error so symlink rewrites are not load-bearing).
/// 2. Strip the leading `/`.
/// 3. Replace each remaining `/` with `-`.
/// 4. Replace any character not in `[A-Za-z0-9._-]` with `_` (so Windows
///    `\` and `:` become `_` rather than `-`).
#[must_use]
pub fn sanitize_workspace(p: &Path) -> String {
    let canonical = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = canonical.to_string_lossy();

    let trimmed = s.trim_start_matches('/').to_string();

    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch == '/' {
            out.push('-');
        } else if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn s(p: &str) -> String {
        // Use a path that won't exist, so canonicalize falls back to the input.
        sanitize_workspace(&PathBuf::from(p))
    }

    #[test]
    fn replaces_slashes_with_dashes() {
        assert_eq!(s("/Users/jf/dev/caliban"), "Users-jf-dev-caliban");
    }

    #[test]
    fn drops_leading_separator() {
        let out = s("/a/b/c");
        assert!(!out.starts_with('-'), "got: {out}");
        assert_eq!(out, "a-b-c");
    }

    #[test]
    fn replaces_unsafe_chars_with_underscore() {
        assert_eq!(s("/home/jf/work/foo bar"), "home-jf-work-foo_bar");
        // Windows-shaped path: ':' and '\\' replaced.
        assert_eq!(s("C:\\src\\proj"), "C__src_proj");
    }

    #[test]
    fn is_idempotent() {
        let once = s("/a/b/c/d");
        let twice = sanitize_workspace(&PathBuf::from(&once));
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_dots_underscores_dashes() {
        let out = s("/proj/my.app_v1-rc2");
        assert_eq!(out, "proj-my.app_v1-rc2");
    }
}
