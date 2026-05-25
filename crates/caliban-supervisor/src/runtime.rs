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
}
