//! XDG / OS path helpers, workspace-path sanitization, and ancestor-walk
//! file discovery.
//!
//! These are the small bits of path plumbing that several caliban crates
//! had reinvented — consolidated here so the discovery rules stay
//! consistent across the workspace.

use std::path::{Path, PathBuf};

/// Resolve the per-app XDG config home for `app`.
///
/// 1. `$XDG_CONFIG_HOME/<app>` if `XDG_CONFIG_HOME` is set and non-empty.
/// 2. Else `$HOME/.config/<app>`.
/// 3. Else (no `HOME`) returns the relative path `.config/<app>` — callers
///    deal with the absent-home edge case explicitly.
///
/// macOS callers still honor `XDG_CONFIG_HOME` so the test suite and
/// operator overrides behave the same as on Linux.
#[must_use]
pub fn xdg_config_home(app: &str) -> PathBuf {
    if let Ok(custom) = std::env::var("XDG_CONFIG_HOME")
        && !custom.is_empty()
    {
        return PathBuf::from(custom).join(app);
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".config").join(app);
    }
    PathBuf::from(".config").join(app)
}

/// Resolve the per-app XDG data home for `app`.
///
/// 1. `$XDG_DATA_HOME/<app>` if `XDG_DATA_HOME` is set and non-empty.
/// 2. Else `$HOME/.local/share/<app>`.
/// 3. Else falls back to a relative path `.local/share/<app>`.
#[must_use]
pub fn xdg_data_home(app: &str) -> PathBuf {
    if let Ok(custom) = std::env::var("XDG_DATA_HOME")
        && !custom.is_empty()
    {
        return PathBuf::from(custom).join(app);
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".local").join("share").join(app);
    }
    PathBuf::from(".local").join("share").join(app)
}

/// Per-app XDG runtime dir, or `None` when `XDG_RUNTIME_DIR` is unset.
///
/// macOS doesn't set `XDG_RUNTIME_DIR` by default — callers are expected
/// to handle the `None` case (typically by falling back to `xdg_data_home`
/// or a tempdir).
#[must_use]
pub fn xdg_runtime_home(app: &str) -> Option<PathBuf> {
    let raw = std::env::var("XDG_RUNTIME_DIR").ok()?;
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw).join(app))
}

/// Build a directory-safe slug from an absolute workspace path.
///
/// Rules:
/// 1. Canonicalize via [`std::fs::canonicalize`] (best-effort; fall back to
///    the original path on error so symlink rewrites are not load-bearing).
/// 2. Strip the leading `/`.
/// 3. Replace each remaining `/` with `-`.
/// 4. Replace any character not in `[A-Za-z0-9._-]` with `_` (so Windows
///    `\` and `:` become `_` rather than `-`).
///
/// Output is suitable for paths like `~/.caliban/projects/<sanitized>/`.
#[must_use]
pub fn sanitize_cwd_for_path(cwd: &Path) -> String {
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
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
        // Check after the candidate so a config at the git root is still
        // discoverable.
        if dir.join(".git").exists() {
            return None;
        }
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

    // `std::env::set_var` / `remove_var` were marked `unsafe` in Rust 2024
    // because mutating the process environment is racy with other threads.
    // The workspace lint denies `unsafe_code`; we localize the `#[allow]` to
    // this test-only helper, restore the previous value on drop, and accept
    // the documented race in single-threaded `cargo test` runs.
    //
    // SAFETY: see comment above.
    #[allow(unsafe_code)]
    fn set_env(key: &str, value: Option<&str>) {
        match value {
            // SAFETY: see module-level comment above.
            Some(v) => unsafe { std::env::set_var(key, v) },
            // SAFETY: see module-level comment above.
            None => unsafe { std::env::remove_var(key) },
        }
    }

    /// Process-wide mutex serializing env-mutating tests. Cargo runs unit
    /// tests in parallel by default; concurrent `set_var` / `remove_var`
    /// calls race regardless of how careful any single test is, so the
    /// mutex is held across the full lifetime of each `EnvGuard`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard restoring `key` to its prior value on drop. Acquires
    /// the process-wide [`ENV_LOCK`] for the guard's lifetime so other
    /// env-mutating tests serialize behind it.
    struct EnvGuard {
        key: String,
        prev: Option<String>,
        // Held for the lifetime of the guard. Poison is ignored — env
        // restoration on Drop is best-effort.
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl EnvGuard {
        fn set(key: &str, val: Option<&str>) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let prev = std::env::var(key).ok();
            set_env(key, val);
            Self {
                key: key.into(),
                prev,
                _lock: lock,
            }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            set_env(&self.key, self.prev.as_deref());
        }
    }

    // --- xdg_config_home / xdg_data_home ---

    #[test]
    fn xdg_config_home_honors_env() {
        let _g = EnvGuard::set("XDG_CONFIG_HOME", Some("/tmp/cfg"));
        let p = xdg_config_home("caliban");
        assert_eq!(p, PathBuf::from("/tmp/cfg/caliban"));
    }

    #[test]
    fn xdg_config_home_falls_back_when_env_unset() {
        let _g = EnvGuard::set("XDG_CONFIG_HOME", None);
        let p = xdg_config_home("caliban");
        if let Some(home) = dirs::home_dir() {
            assert_eq!(p, home.join(".config").join("caliban"));
        } else {
            assert_eq!(p, PathBuf::from(".config").join("caliban"));
        }
    }

    #[test]
    fn xdg_data_home_honors_env() {
        let _g = EnvGuard::set("XDG_DATA_HOME", Some("/tmp/data"));
        let p = xdg_data_home("caliban");
        assert_eq!(p, PathBuf::from("/tmp/data/caliban"));
    }

    #[test]
    fn xdg_runtime_home_returns_none_when_env_unset() {
        let _g = EnvGuard::set("XDG_RUNTIME_DIR", None);
        let p = xdg_runtime_home("caliban");
        assert!(p.is_none(), "got: {p:?}");
    }

    #[test]
    fn xdg_runtime_home_returns_some_when_env_set() {
        let _g = EnvGuard::set("XDG_RUNTIME_DIR", Some("/run/user/1000"));
        let p = xdg_runtime_home("caliban");
        assert_eq!(p, Some(PathBuf::from("/run/user/1000/caliban")));
    }

    // --- sanitize_cwd_for_path ---

    #[test]
    fn sanitize_replaces_slashes_with_dashes() {
        assert_eq!(
            sanitize_cwd_for_path(Path::new("/Users/jf/dev/caliban")),
            "Users-jf-dev-caliban"
        );
    }

    #[test]
    fn sanitize_replaces_unsafe_chars_with_underscore() {
        assert_eq!(
            sanitize_cwd_for_path(Path::new("/home/jf/work/foo bar")),
            "home-jf-work-foo_bar"
        );
    }

    #[test]
    fn sanitize_preserves_dots_underscores_dashes() {
        assert_eq!(
            sanitize_cwd_for_path(Path::new("/proj/my.app_v1-rc2")),
            "proj-my.app_v1-rc2"
        );
    }

    // --- walk_up_for_file ---

    #[test]
    fn walk_up_finds_file_in_starting_dir() {
        let tmp = tempdir().unwrap();
        let f = tmp.path().join("caliban.toml");
        fs::write(&f, "x").unwrap();
        let found = walk_up_for_file(tmp.path(), "caliban.toml").unwrap();
        assert_eq!(found.canonicalize().unwrap(), f.canonicalize().unwrap());
    }

    #[test]
    fn walk_up_walks_to_ancestor() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        let f = tmp.path().join("caliban.toml");
        fs::write(&f, "x").unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let found = walk_up_for_file(&nested, "caliban.toml").unwrap();
        assert_eq!(found.canonicalize().unwrap(), f.canonicalize().unwrap());
    }

    #[test]
    fn walk_up_stops_at_git_root() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("sub");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        // file outside the git root — must not be found.
        let outside = tmp.path().parent().unwrap().join("caliban.toml.outside");
        let _ = fs::write(&outside, "x");
        assert!(walk_up_for_file(&nested, "caliban.toml").is_none());
    }
}
