//! Runtime-directory + per-repo socket path resolution.
//!
//! Per the design spec: socket path is
//! `${CALIBAN_DAEMON_RUNTIME_DIR:-$XDG_RUNTIME_DIR/caliban}/<hash(repo_root)>.sock`
//! (and we fall back to `$TMPDIR/caliban-daemon` when neither env var is
//! set — primarily macOS, where `$XDG_RUNTIME_DIR` isn't conventional).

use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// Compute a stable, short hash of the absolute repo root. We use the
/// first 16 hex chars of SHA-256, which collides at a rate that's
/// irrelevant in practice (one machine, few repos).
#[must_use]
pub fn repo_hash(repo_root: &Path) -> String {
    use std::fmt::Write as _;
    let s = repo_root.to_string_lossy();
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let bytes = h.finalize();
    // 8 bytes → 16 hex chars.
    let mut out = String::with_capacity(16);
    for b in &bytes[..8] {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Resolve the default runtime directory for caliban daemon sockets.
///
/// Resolution order:
/// 1. `$CALIBAN_DAEMON_RUNTIME_DIR` if set.
/// 2. `$XDG_RUNTIME_DIR/caliban/` if `$XDG_RUNTIME_DIR` is set.
/// 3. `$TMPDIR/caliban-daemon/`.
#[must_use]
pub fn default_runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CALIBAN_DAEMON_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("caliban");
    }
    std::env::temp_dir().join("caliban-daemon")
}

/// Compute the per-repo daemon socket path under the default runtime dir.
#[must_use]
pub fn repo_socket_path(repo_root: &Path) -> PathBuf {
    repo_socket_path_in(&default_runtime_dir(), repo_root)
}

/// Compute the per-repo daemon socket path under an explicit runtime
/// directory (used by tests).
#[must_use]
pub fn repo_socket_path_in(runtime_dir: &Path, repo_root: &Path) -> PathBuf {
    runtime_dir.join(format!("{}.sock", repo_hash(repo_root)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_hash_stable() {
        let a = repo_hash(Path::new("/tmp/foo"));
        let b = repo_hash(Path::new("/tmp/foo"));
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn repo_hash_differs_per_path() {
        assert_ne!(repo_hash(Path::new("/a")), repo_hash(Path::new("/b")));
    }

    #[test]
    fn socket_path_in_runtime_dir() {
        let dir = PathBuf::from("/tmp/runtime");
        let p = repo_socket_path_in(&dir, Path::new("/repo"));
        assert!(p.starts_with("/tmp/runtime"));
        assert!(p.extension().is_some_and(|e| e == "sock"));
    }

    #[test]
    fn repo_hash_is_lowercase_hex() {
        let h = repo_hash(Path::new("/some/repo/root"));
        assert_eq!(h.len(), 16);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    #[test]
    fn socket_filename_is_repo_hash_dot_sock() {
        let dir = PathBuf::from("/tmp/rt");
        let repo = Path::new("/repo/here");
        let p = repo_socket_path_in(&dir, repo);
        let expected = format!("{}.sock", repo_hash(repo));
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), expected);
    }

    // The env-mutating tests below share process-global state
    // (`std::env::set_var`), so they must not run concurrently with each
    // other. A static mutex serializes them; each restores prior values.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        #[allow(unsafe_code)]
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: all env mutation in these tests is serialized by
            // ENV_LOCK, so no other thread observes a torn read/write.
            unsafe { std::env::set_var(key, val) };
            Self { key, prev }
        }

        #[allow(unsafe_code)]
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: all env mutation in these tests is serialized by
            // ENV_LOCK, so no other thread observes a torn read/write.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: all env mutation in these tests is serialized by
            // ENV_LOCK, so no other thread observes a torn read/write.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn default_runtime_dir_prefers_caliban_env() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::set("CALIBAN_DAEMON_RUNTIME_DIR", "/custom/rt");
        let _x = EnvGuard::set("XDG_RUNTIME_DIR", "/xdg");
        assert_eq!(default_runtime_dir(), PathBuf::from("/custom/rt"));
    }

    #[test]
    fn default_runtime_dir_falls_back_to_xdg() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("CALIBAN_DAEMON_RUNTIME_DIR");
        let _x = EnvGuard::set("XDG_RUNTIME_DIR", "/xdg-run");
        assert_eq!(default_runtime_dir(), PathBuf::from("/xdg-run/caliban"));
    }

    #[test]
    fn default_runtime_dir_ignores_empty_caliban_env() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::set("CALIBAN_DAEMON_RUNTIME_DIR", "");
        let _x = EnvGuard::set("XDG_RUNTIME_DIR", "/xdg-run2");
        // Empty caliban var must be skipped, falling through to XDG.
        assert_eq!(default_runtime_dir(), PathBuf::from("/xdg-run2/caliban"));
    }

    #[test]
    fn default_runtime_dir_ignores_empty_xdg() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("CALIBAN_DAEMON_RUNTIME_DIR");
        let _x = EnvGuard::set("XDG_RUNTIME_DIR", "");
        // Both unusable -> temp_dir based fallback.
        let got = default_runtime_dir();
        assert_eq!(got, std::env::temp_dir().join("caliban-daemon"));
    }

    #[test]
    fn default_runtime_dir_falls_back_to_tmp() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("CALIBAN_DAEMON_RUNTIME_DIR");
        let _x = EnvGuard::unset("XDG_RUNTIME_DIR");
        assert_eq!(
            default_runtime_dir(),
            std::env::temp_dir().join("caliban-daemon")
        );
    }

    #[test]
    fn repo_socket_path_uses_default_runtime_dir() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::set("CALIBAN_DAEMON_RUNTIME_DIR", "/rt/base");
        let repo = Path::new("/some/repo");
        let p = repo_socket_path(repo);
        assert_eq!(
            p,
            PathBuf::from("/rt/base").join(format!("{}.sock", repo_hash(repo)))
        );
    }
}
