//! Pruning policy: drop session checkpoint directories whose newest manifest
//! is older than `cleanupPeriodDays` (default 30).
//!
//! Wired by the session-pruning sweep — keep `caliban-checkpoint` orphans
//! from accumulating after their parent session is removed.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::error::{CheckpointError, Result};

/// Default retention period in days.
pub const DEFAULT_RETENTION_DAYS: u64 = 30;

/// Read the `cleanupPeriodDays` env override; falls back to the default.
#[must_use]
pub fn retention_days() -> u64 {
    std::env::var("CALIBAN_CLEANUP_PERIOD_DAYS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Walk `<root>/projects/<sanitized-cwd>/checkpoints/` and remove any
/// `<session_id>/` subtree whose last-modified time is older than the
/// retention threshold.
///
/// Returns the number of session directories removed.
///
/// # Errors
/// I/O errors reading the root or removing directories.
pub fn prune_root(checkpoints_root: &Path, retention: Duration) -> Result<usize> {
    let entries = match std::fs::read_dir(checkpoints_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(CheckpointError::Io(e)),
    };
    let mut removed = 0;
    let now = SystemTime::now();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Most recent prompt's mtime, or the dir's mtime if empty.
        let last_seen = newest_mtime(&path).unwrap_or_else(|| {
            std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(now)
        });
        if let Ok(age) = now.duration_since(last_seen)
            && age > retention
        {
            std::fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<SystemTime> = None;
    for entry in entries.flatten() {
        let meta = entry.metadata().ok()?;
        let modified = meta.modified().ok()?;
        best = Some(match best {
            Some(prev) if prev > modified => prev,
            _ => modified,
        });
    }
    best
}

#[cfg(test)]
#[allow(
    clippy::duration_suboptimal_units,
    reason = "test fixtures favor explicit s*m*h math"
)]
mod tests {
    use super::*;
    use crate::CheckpointStore;
    use crate::manifest::{Manifest, ManifestKind};
    use filetime::{FileTime, set_file_mtime};
    use tempfile::TempDir;

    fn write_old(path: &Path, days_old: u64) {
        let now = std::time::SystemTime::now();
        let target = now - Duration::from_secs(60 * 60 * 24 * days_old);
        let ft = FileTime::from_system_time(target);
        set_file_mtime(path, ft).expect("set mtime");
    }

    // We can't always rely on filetime; if unavailable, the prune test
    // becomes a smoke test of the "nothing to remove" path.
    #[test]
    fn prune_recent_checkpoints_keeps_them() {
        let tmp = TempDir::new().unwrap();
        let store_root = tmp.path().to_path_buf();
        let cp = CheckpointStore::open_in(&store_root, Path::new("/cwd/example"), "sess-recent")
            .unwrap();
        cp.save_manifest(&Manifest::new(1, ManifestKind::Files, "p"))
            .unwrap();
        let cp_root = store_root
            .join("projects")
            .join(crate::sanitize_cwd(Path::new("/cwd/example")))
            .join("checkpoints");
        let removed = prune_root(&cp_root, Duration::from_secs(60 * 60 * 24 * 30)).unwrap();
        assert_eq!(removed, 0);
        assert!(cp.session_dir().exists());
    }

    #[test]
    fn prune_old_checkpoints_removes_them() {
        let tmp = TempDir::new().unwrap();
        let store_root = tmp.path().to_path_buf();
        let cp =
            CheckpointStore::open_in(&store_root, Path::new("/cwd/example"), "sess-old").unwrap();
        cp.save_manifest(&Manifest::new(1, ManifestKind::Files, "p"))
            .unwrap();
        // Backdate every file in the session dir 100 days.
        backdate_recursively(cp.session_dir(), 100);
        let cp_root = store_root
            .join("projects")
            .join(crate::sanitize_cwd(Path::new("/cwd/example")))
            .join("checkpoints");
        let removed = prune_root(&cp_root, Duration::from_secs(60 * 60 * 24 * 30)).unwrap();
        assert_eq!(removed, 1);
        assert!(!cp.session_dir().exists());
    }

    fn backdate_recursively(dir: &Path, days: u64) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    backdate_recursively(&p, days);
                } else {
                    write_old(&p, days);
                }
            }
        }
        write_old(dir, days);
    }
}
