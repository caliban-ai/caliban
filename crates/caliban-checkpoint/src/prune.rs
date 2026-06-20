//! Pruning policy: drop session checkpoint directories whose newest manifest
//! is older than `cleanupPeriodDays` (default 30).
//!
//! Wired by the session-pruning sweep — keep `caliban-checkpoint` orphans
//! from accumulating after their parent session is removed.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};

use crate::error::{CheckpointError, Result};
use crate::manifest::{Manifest, ManifestKind};

/// Default retention period in days.
pub const DEFAULT_RETENTION_DAYS: u64 = 30;

/// Default per-project checkpoint blob byte-cap (5 GiB).
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Read the `cleanupPeriodDays` env override; falls back to the default.
#[must_use]
pub fn retention_days() -> u64 {
    std::env::var("CALIBAN_CLEANUP_PERIOD_DAYS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Read the `CALIBAN_CHECKPOINT_MAX_BYTES` env override; falls back to the
/// 5 GiB default.
#[must_use]
pub fn checkpoint_max_bytes() -> u64 {
    std::env::var("CALIBAN_CHECKPOINT_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_MAX_BYTES)
}

/// One blob-bearing prompt directory, for byte-cap accounting.
struct PromptBlobs {
    prompt_dir: PathBuf,
    bytes: u64,
    created_at: DateTime<Utc>,
}

/// `true` only if `path` is a *real* directory — not a symlink to one. Using
/// `symlink_metadata` keeps the byte-cap sweep from following a symlink out of
/// the checkpoint tree, where it would count foreign bytes or risk deleting
/// outside the tree (#220 issue 4).
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Sum the size of the regular files directly inside `dir` (the per-prompt
/// `blobs/` directory). Returns 0 if the directory is missing or is a symlink.
/// `DirEntry::metadata()` does not traverse symlinked entries, and the `dir`
/// itself is gated by [`is_real_dir`], so no symlink is ever followed (#220).
fn dir_file_bytes(dir: &Path) -> u64 {
    if !is_real_dir(dir) {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for e in entries.flatten() {
        if let Ok(meta) = e.metadata()
            && meta.is_file()
        {
            total += meta.len();
        }
    }
    total
}

fn read_manifest(prompt_dir: &Path) -> Option<Manifest> {
    let body = std::fs::read(prompt_dir.join("manifest.json")).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Enforce the per-project checkpoint blob byte-cap (ADR-0028). Sums all blob
/// bytes under `<checkpoints_root>/<session>/prompt-*/blobs/`; when the total
/// exceeds `max_bytes`, evicts the **oldest** prompts' blobs — deleting each
/// `blobs/` directory and rewriting the prompt's manifest to
/// [`ManifestKind::Cleared`] with empty `entries` — until the total is back
/// under the cap. Returns the number of prompts cleared.
///
/// `exclude` names a prompt directory that must never be swept — the caller
/// passes the prompt it just wrote, so a single prompt larger than the cap
/// can't self-destruct (the sweep ran in `close_prompt` right after writing it
/// with no exclusion: #220 issue 1). Pass `None` to consider every prompt.
///
/// Implements the documented `CALIBAN_CHECKPOINT_MAX_BYTES` behavior that was
/// previously inert (#180): pruning was age-only, so blob storage grew
/// unbounded.
///
/// Note (#220 issue 3): the cap spans the project-wide `checkpoints/` root
/// across all sessions with no inter-process lock, so concurrent sub-agents /
/// `/fork` / parallel TUI sweeps can mis-account or race manifest rewrites. The
/// sweep is therefore best-effort; precise cross-session accounting needs a
/// root-level lock, tracked as follow-up before the hook is wired.
///
/// # Errors
/// I/O errors deleting a `blobs/` directory or rewriting a manifest.
pub fn enforce_byte_cap(
    checkpoints_root: &Path,
    max_bytes: u64,
    exclude: Option<&Path>,
) -> Result<usize> {
    let sessions = match std::fs::read_dir(checkpoints_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(CheckpointError::Io(e)),
    };
    let mut prompts: Vec<PromptBlobs> = Vec::new();
    let mut total: u64 = 0;
    for session in sessions.flatten() {
        let sdir = session.path();
        if !is_real_dir(&sdir) {
            continue;
        }
        let Ok(prompt_dirs) = std::fs::read_dir(&sdir) else {
            continue;
        };
        for pd in prompt_dirs.flatten() {
            let pdir = pd.path();
            if !is_real_dir(&pdir) {
                continue;
            }
            if exclude.is_some_and(|ex| ex == pdir) {
                continue; // never evict the prompt the caller just wrote
            }
            let bytes = dir_file_bytes(&pdir.join("blobs"));
            if bytes == 0 {
                continue; // plan / already-cleared / empty prompt
            }
            total += bytes;
            // Order by the manifest's created_at, falling back to the dir mtime.
            let created_at = read_manifest(&pdir).map_or_else(
                || {
                    std::fs::metadata(&pdir)
                        .and_then(|m| m.modified())
                        .map_or_else(|_| Utc::now(), DateTime::<Utc>::from)
                },
                |m| m.created_at,
            );
            prompts.push(PromptBlobs {
                prompt_dir: pdir,
                bytes,
                created_at,
            });
        }
    }
    if total <= max_bytes {
        return Ok(0);
    }
    // Evict oldest-first until back under the cap.
    prompts.sort_by_key(|p| p.created_at);
    let mut cleared = 0;
    for p in prompts {
        if total <= max_bytes {
            break;
        }
        let blobs = p.prompt_dir.join("blobs");
        // `is_real_dir` already gated accounting; re-check before deleting so a
        // `blobs/` swapped for a symlink between scan and delete is never
        // followed by `remove_dir_all` (#220 issue 4).
        if !is_real_dir(&blobs) {
            continue;
        }
        std::fs::remove_dir_all(&blobs).map_err(CheckpointError::Io)?;
        total = total.saturating_sub(p.bytes);
        if let Some(mut m) = read_manifest(&p.prompt_dir) {
            m.kind = ManifestKind::Cleared;
            m.entries.clear();
            let body = serde_json::to_vec_pretty(&m)?;
            caliban_common::fs::write_atomic(&p.prompt_dir.join("manifest.json"), &body)
                .map_err(CheckpointError::Io)?;
        }
        cleared += 1;
    }
    Ok(cleared)
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

    #[test]
    fn byte_cap_evicts_oldest_blobs_and_marks_cleared() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let cp = CheckpointStore::open_in(&root, Path::new("/cwd/cap"), "sess-cap").unwrap();

        // Prompt 1 (older): a 3000-byte blob + a manifest entry referencing it.
        cp.write_blob(1, "aaaa", &vec![1u8; 3000]).unwrap();
        let mut m1 = Manifest::new(1, ManifestKind::Files, "old");
        m1.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
        m1.entries.push(crate::manifest::ManifestEntry {
            path: "/x".into(),
            blob_sha256: "aaaa".into(),
            mode: 0o644,
            size: 3000,
            exists_pre: true,
            tool_name: "Write".into(),
            tool_use_id: String::new(),
            error: None,
        });
        cp.save_manifest(&m1).unwrap();

        // Prompt 2 (newer): a 3000-byte blob.
        cp.write_blob(2, "bbbb", &vec![2u8; 3000]).unwrap();
        let mut m2 = Manifest::new(2, ManifestKind::Files, "new");
        m2.created_at = "2024-01-01T00:00:00Z".parse().unwrap();
        cp.save_manifest(&m2).unwrap();

        // Total 6000 bytes; cap 4000 → evict only the oldest prompt.
        let checkpoints_root = cp.session_dir().parent().unwrap();
        let cleared = enforce_byte_cap(checkpoints_root, 4000, None).unwrap();
        assert_eq!(cleared, 1, "exactly the oldest prompt is cleared");

        // Prompt 1: blobs gone, manifest rewritten to Cleared with no entries.
        assert!(!cp.blobs_dir(1).exists(), "old blobs evicted");
        let m1_after = cp.load_manifest(1).unwrap();
        assert_eq!(m1_after.kind, ManifestKind::Cleared);
        assert!(m1_after.entries.is_empty());

        // Prompt 2: retained untouched.
        assert!(cp.blobs_dir(2).exists(), "newest blobs retained");
        assert_eq!(cp.load_manifest(2).unwrap().kind, ManifestKind::Files);

        // Idempotent: a second sweep under the cap clears nothing.
        assert_eq!(enforce_byte_cap(checkpoints_root, 4000, None).unwrap(), 0);
    }

    #[test]
    fn byte_cap_never_evicts_the_excluded_active_prompt() {
        // #220 issue 1: a single prompt larger than the cap must not be evicted
        // when it is the active (just-written) prompt.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let cp = CheckpointStore::open_in(&root, Path::new("/cwd/cap1"), "sess-1").unwrap();
        cp.write_blob(1, "aaaa", &vec![1u8; 9000]).unwrap();
        cp.save_manifest(&Manifest::new(1, ManifestKind::Files, "only"))
            .unwrap();

        let checkpoints_root = cp.session_dir().parent().unwrap();
        // Cap 4000, but prompt-001 is the excluded active prompt → nothing evicted.
        let cleared = enforce_byte_cap(checkpoints_root, 4000, Some(&cp.prompt_dir(1))).unwrap();
        assert_eq!(
            cleared, 0,
            "the active over-cap prompt is never self-evicted"
        );
        assert!(cp.blobs_dir(1).exists(), "active prompt blobs preserved");

        // Without the exclusion the same prompt *would* be swept.
        let cleared_unguarded = enforce_byte_cap(checkpoints_root, 4000, None).unwrap();
        assert_eq!(cleared_unguarded, 1);
    }

    #[test]
    fn byte_cap_ignores_symlinked_blobs_dir() {
        // #220 issue 4: a symlinked `blobs/` must not be followed (counted or
        // deleted) by the sweep.
        #[cfg(unix)]
        {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().to_path_buf();
            let cp = CheckpointStore::open_in(&root, Path::new("/cwd/sym"), "sess-sym").unwrap();
            cp.ensure_prompt_dir(1).unwrap();
            cp.save_manifest(&Manifest::new(1, ManifestKind::Files, "p"))
                .unwrap();
            // Point prompt-001/blobs at a foreign directory full of big files.
            let foreign = tmp.path().join("foreign");
            std::fs::create_dir_all(&foreign).unwrap();
            std::fs::write(foreign.join("big.bin"), vec![7u8; 100_000]).unwrap();
            let blobs_link = cp.prompt_dir(1).join("blobs");
            let _ = std::fs::remove_dir_all(&blobs_link);
            std::os::unix::fs::symlink(&foreign, &blobs_link).unwrap();

            let checkpoints_root = cp.session_dir().parent().unwrap();
            // Tiny cap: if the symlink were followed, the 100 KiB foreign file
            // would blow the cap and the sweep would try to delete it.
            let cleared = enforce_byte_cap(checkpoints_root, 10, None).unwrap();
            assert_eq!(
                cleared, 0,
                "symlinked blobs are neither counted nor deleted"
            );
            assert!(foreign.join("big.bin").exists(), "foreign tree untouched");
        }
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
