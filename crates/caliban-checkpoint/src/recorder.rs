//! In-flight per-prompt checkpoint state.
//!
//! [`CheckpointRecorder`] is the cooperating type that hook impls drive: it
//! owns the current prompt's `Manifest`, a `BTreeMap<canonical_path, Entry>`
//! keyed on the file's first writer, and the [`crate::CheckpointStore`]
//! handle.
//!
//! The contract is small:
//!
//! - [`Self::open_prompt`] allocates a fresh manifest + prompt directory.
//! - [`Self::capture`] reads the pre-image of `path` (or marks the entry
//!   as `exists_pre: false` when it doesn't exist), writes the blob,
//!   and records the entry. First writer wins on a per-path basis —
//!   subsequent captures within the same prompt are no-ops.
//! - [`Self::record_last_message_id`] / [`Self::mark_partial`] mutate
//!   metadata.
//! - [`Self::close_prompt`] flushes the manifest to disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::error::{CheckpointError, Result};
use crate::manifest::{Manifest, ManifestEntry, ManifestKind};
use crate::store::CheckpointStore;

/// Maximum pre-image size we'll attempt to capture, in bytes. Files larger
/// than this are recorded with `error: Some(...)` and cannot be restored.
/// Override via `CALIBAN_CHECKPOINT_MAX_FILE_BYTES`.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

fn max_file_bytes() -> u64 {
    std::env::var("CALIBAN_CHECKPOINT_MAX_FILE_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_FILE_BYTES)
}

/// Recorder state for an open prompt.
struct OpenPrompt {
    manifest: Manifest,
    // Keep per-path entries keyed by canonical path for first-writer-wins
    // semantics under parallel tool dispatch.
    entries: BTreeMap<PathBuf, usize>, // path → index in manifest.entries
}

/// Drives a [`CheckpointStore`] across a prompt boundary.
///
/// Cheap to clone — wraps an `Arc<Mutex<...>>`.
#[derive(Clone)]
pub struct CheckpointRecorder {
    store: CheckpointStore,
    workspace_root: PathBuf,
    inner: Arc<Mutex<Option<OpenPrompt>>>,
}

impl std::fmt::Debug for CheckpointRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckpointRecorder")
            .field("store", &self.store)
            .field("workspace_root", &self.workspace_root)
            .finish_non_exhaustive()
    }
}

impl CheckpointRecorder {
    /// Construct a recorder around a store + workspace root.
    #[must_use]
    pub fn new(store: CheckpointStore, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            store,
            workspace_root: workspace_root.into(),
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Access the underlying store.
    #[must_use]
    pub fn store(&self) -> &CheckpointStore {
        &self.store
    }

    /// Start a new prompt. Allocates the on-disk directory tree and stashes
    /// an empty manifest in memory.
    ///
    /// # Errors
    /// I/O errors creating the prompt directory.
    pub async fn open_prompt(
        &self,
        prompt_index: u32,
        kind: ManifestKind,
        title: impl Into<String>,
    ) -> Result<()> {
        self.store.ensure_prompt_dir(prompt_index)?;
        let mut guard = self.inner.lock().await;
        *guard = Some(OpenPrompt {
            manifest: Manifest::new(prompt_index, kind, title),
            entries: BTreeMap::new(),
        });
        Ok(())
    }

    /// Capture `path`'s pre-image into the active prompt manifest.
    ///
    /// First writer wins per path. Non-workspace paths are rejected with
    /// [`CheckpointError::Skipped`]. Read failures are recorded as entries
    /// with `error: Some(...)` so they surface in the manifest but never
    /// halt the tool.
    ///
    /// # Errors
    /// I/O while reading the pre-image or writing the blob. The skip
    /// variant is also returned for non-workspace paths.
    #[allow(
        clippy::too_many_lines,
        reason = "single state machine — splitting would obscure the per-error-branch shape"
    )]
    pub async fn capture(&self, path: &Path, tool_name: &str, tool_use_id: &str) -> Result<()> {
        // Skip non-workspace paths (matches Claude Code). Canonicalize as
        // much of the path as exists — files Write creates from scratch
        // don't canonicalize on their own.
        let canonical = canonicalize_existing_ancestor(path);
        if !canonical.starts_with(&self.workspace_root) {
            return Err(CheckpointError::Skipped {
                reason: format!("non-workspace path: {}", canonical.display()),
            });
        }

        let mut guard = self.inner.lock().await;
        let Some(open) = guard.as_mut() else {
            return Err(CheckpointError::Skipped {
                reason: "no open prompt".into(),
            });
        };
        // First-writer wins: skip if we already have this path.
        if open.entries.contains_key(&canonical) {
            return Ok(());
        }

        let prompt_index = open.manifest.prompt_index;
        // Read the pre-image.
        match std::fs::metadata(&canonical) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File didn't exist → restore should delete it.
                let entry = ManifestEntry {
                    path: canonical.clone(),
                    blob_sha256: String::new(),
                    mode: 0o644,
                    size: 0,
                    exists_pre: false,
                    tool_name: tool_name.into(),
                    tool_use_id: tool_use_id.into(),
                    error: None,
                };
                let idx = open.manifest.entries.len();
                open.manifest.entries.push(entry);
                open.entries.insert(canonical, idx);
                Ok(())
            }
            Err(e) => {
                let entry = ManifestEntry {
                    path: canonical.clone(),
                    blob_sha256: String::new(),
                    mode: 0o644,
                    size: 0,
                    exists_pre: true,
                    tool_name: tool_name.into(),
                    tool_use_id: tool_use_id.into(),
                    error: Some(format!("metadata: {e}")),
                };
                let idx = open.manifest.entries.len();
                open.manifest.entries.push(entry);
                open.entries.insert(canonical, idx);
                open.manifest.partial = true;
                Ok(())
            }
            Ok(md) => {
                let size = md.len();
                let mode = unix_mode(&md);
                if size > max_file_bytes() {
                    let entry = ManifestEntry {
                        path: canonical.clone(),
                        blob_sha256: String::new(),
                        mode,
                        size,
                        exists_pre: true,
                        tool_name: tool_name.into(),
                        tool_use_id: tool_use_id.into(),
                        error: Some(format!(
                            "pre-image exceeds {} bytes (configured cap)",
                            max_file_bytes()
                        )),
                    };
                    let idx = open.manifest.entries.len();
                    open.manifest.entries.push(entry);
                    open.entries.insert(canonical, idx);
                    open.manifest.partial = true;
                    return Ok(());
                }
                let bytes = match std::fs::read(&canonical) {
                    Ok(b) => b,
                    Err(e) => {
                        let entry = ManifestEntry {
                            path: canonical.clone(),
                            blob_sha256: String::new(),
                            mode,
                            size,
                            exists_pre: true,
                            tool_name: tool_name.into(),
                            tool_use_id: tool_use_id.into(),
                            error: Some(format!("read: {e}")),
                        };
                        let idx = open.manifest.entries.len();
                        open.manifest.entries.push(entry);
                        open.entries.insert(canonical, idx);
                        open.manifest.partial = true;
                        return Ok(());
                    }
                };
                let sha = sha256_hex(&bytes);
                self.store.write_blob(prompt_index, &sha, &bytes)?;
                let entry = ManifestEntry {
                    path: canonical.clone(),
                    blob_sha256: sha,
                    mode,
                    size,
                    exists_pre: true,
                    tool_name: tool_name.into(),
                    tool_use_id: tool_use_id.into(),
                    error: None,
                };
                let idx = open.manifest.entries.len();
                open.manifest.entries.push(entry);
                open.entries.insert(canonical, idx);
                Ok(())
            }
        }
    }

    /// Record the last assistant message id seen during this prompt (used
    /// for conversation truncation on restore).
    pub async fn record_last_message_id(&self, id: impl Into<String>) {
        let mut guard = self.inner.lock().await;
        if let Some(open) = guard.as_mut() {
            open.manifest.last_message_id = Some(id.into());
        }
    }

    /// Mark the currently-open manifest as partial.
    pub async fn mark_partial(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(open) = guard.as_mut() {
            open.manifest.partial = true;
        }
    }

    /// Flush the in-memory manifest to disk and clear the open-prompt
    /// state. Returns `Ok(())` even when no prompt was open (no-op).
    ///
    /// # Errors
    /// I/O errors writing the manifest.
    pub async fn close_prompt(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if let Some(open) = guard.take() {
            self.store.save_manifest(&open.manifest)?;
            // Enforce the per-project blob byte-cap now that this prompt's
            // blobs have landed (#180). Best-effort: a sweep failure must not
            // fail the prompt. `session_dir`'s parent is the project's
            // `checkpoints/` root that the cap spans.
            if let Some(checkpoints_root) = self.store.session_dir().parent() {
                let cap = crate::prune::checkpoint_max_bytes();
                if let Err(e) = crate::prune::enforce_byte_cap(checkpoints_root, cap) {
                    tracing::warn!(error = %e, "checkpoint byte-cap sweep failed (non-fatal)");
                }
            }
        }
        Ok(())
    }

    /// Snapshot of the currently open manifest. Useful for tests.
    #[must_use]
    pub async fn snapshot_manifest(&self) -> Option<Manifest> {
        let guard = self.inner.lock().await;
        guard.as_ref().map(|o| o.manifest.clone())
    }
}

/// Compute hex sha256 of a byte slice.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(unix)]
fn unix_mode(md: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    md.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn unix_mode(_md: &std::fs::Metadata) -> u32 {
    0o644
}

/// Canonicalise as much of `p` as exists, then append the leftover tail.
/// Mirrors `caliban-tools-builtin::workspace::canonicalize_existing_ancestor`
/// so capture decisions stay in lock-step with the resolver the tools use.
fn canonicalize_existing_ancestor(p: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(canon) = std::fs::canonicalize(cur) {
            let mut full = canon;
            for seg in tail.iter().rev() {
                full.push(seg);
            }
            return full;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, CheckpointRecorder) {
        let tmp = TempDir::new().unwrap();
        // Workspace lives under tmp so capture() can canonicalise into it.
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let canonical_ws = std::fs::canonicalize(&workspace).unwrap();
        let store_root = tmp.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        let store = CheckpointStore::open_in(&store_root, &canonical_ws, "sess-1").unwrap();
        let rec = CheckpointRecorder::new(store, canonical_ws);
        (tmp, rec)
    }

    #[tokio::test]
    async fn capture_existing_file_writes_blob_and_manifest_entry() {
        let (tmp, rec) = fixture();
        let path = tmp.path().join("workspace").join("a.txt");
        std::fs::write(&path, "hello").unwrap();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.capture(&path, "Write", "tu_1").await.unwrap();
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.entries.len(), 1);
        assert!(m.entries[0].exists_pre);
        assert_eq!(m.entries[0].size, 5);
        // Blob exists and matches.
        let bytes = rec.store.read_blob(1, &m.entries[0].blob_sha256).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[tokio::test]
    async fn capture_missing_file_records_exists_pre_false() {
        let (tmp, rec) = fixture();
        let path = tmp.path().join("workspace").join("brand-new.txt");
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.capture(&path, "Write", "tu_1").await.unwrap();
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.entries.len(), 1);
        assert!(!m.entries[0].exists_pre);
    }

    #[tokio::test]
    async fn capture_dedups_within_prompt() {
        let (tmp, rec) = fixture();
        let path = tmp.path().join("workspace").join("a.txt");
        std::fs::write(&path, "v1").unwrap();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.capture(&path, "Write", "tu_1").await.unwrap();
        // Even if the file changes, the *pre-image* recorded is the first one.
        std::fs::write(&path, "v2-MUCH-LATER").unwrap();
        rec.capture(&path, "Edit", "tu_2").await.unwrap();
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.entries.len(), 1, "first writer wins");
        assert_eq!(m.entries[0].size, 2, "pre-image is the original v1");
        assert_eq!(m.entries[0].tool_use_id, "tu_1");
    }

    #[tokio::test]
    async fn capture_content_addressed_dedups_blobs() {
        let (tmp, rec) = fixture();
        let a = tmp.path().join("workspace").join("a.txt");
        let b = tmp.path().join("workspace").join("b.txt");
        std::fs::write(&a, "same").unwrap();
        std::fs::write(&b, "same").unwrap();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.capture(&a, "Write", "tu_a").await.unwrap();
        rec.capture(&b, "Write", "tu_b").await.unwrap();
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.entries[0].blob_sha256, m.entries[1].blob_sha256);
        // Only one .bin should exist.
        let blobs_dir = rec.store.blobs_dir(1);
        let count = std::fs::read_dir(&blobs_dir).unwrap().count();
        assert_eq!(count, 1, "identical pre-images dedupe to one blob");
    }

    #[tokio::test]
    async fn close_prompt_persists_manifest() {
        let (tmp, rec) = fixture();
        let path = tmp.path().join("workspace").join("a.txt");
        std::fs::write(&path, "hello").unwrap();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.capture(&path, "Write", "tu_1").await.unwrap();
        rec.close_prompt().await.unwrap();
        let loaded = rec.store.load_manifest(1).unwrap();
        assert_eq!(loaded.entries.len(), 1);
    }

    #[tokio::test]
    async fn rejects_non_workspace_paths() {
        let (tmp, rec) = fixture();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        let outside = tmp.path().join("workspace_outside.txt");
        std::fs::write(&outside, "x").unwrap();
        let err = rec.capture(&outside, "Write", "tu_1").await.unwrap_err();
        assert!(matches!(err, CheckpointError::Skipped { .. }));
    }

    #[tokio::test]
    async fn capture_without_open_prompt_is_skipped() {
        let (tmp, rec) = fixture();
        let path = tmp.path().join("workspace").join("a.txt");
        std::fs::write(&path, "hello").unwrap();
        // No open_prompt() call → capture must skip with a clear reason.
        let err = rec.capture(&path, "Write", "tu_1").await.unwrap_err();
        match err {
            CheckpointError::Skipped { reason } => {
                assert!(reason.contains("no open prompt"), "reason was: {reason}");
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn capture_oversized_file_records_error_and_marks_partial() {
        let (tmp, rec) = fixture();
        let path = tmp.path().join("workspace").join("big.bin");
        // Write a file strictly larger than the default 16 MiB cap so the
        // `size > max_file_bytes()` branch fires. We avoid mutating the env
        // override (the crate forbids `unsafe`, and env is process-global) by
        // exceeding the real default instead.
        let big = vec![0u8; usize::try_from(DEFAULT_MAX_FILE_BYTES + 1).unwrap()];
        std::fs::write(&path, &big).unwrap();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.capture(&path, "Write", "tu_big").await.unwrap();
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.entries.len(), 1);
        let e = &m.entries[0];
        assert!(e.exists_pre, "oversized file still existed pre-prompt");
        assert!(e.blob_sha256.is_empty(), "no blob is written for oversize");
        assert_eq!(e.size, DEFAULT_MAX_FILE_BYTES + 1);
        let msg = e.error.as_deref().unwrap_or_default();
        assert!(msg.contains("exceeds"), "error text was: {msg}");
        assert!(m.partial, "oversized capture marks the manifest partial");
        // No blob should have been written.
        let blobs_dir = rec.store.blobs_dir(1);
        let count = std::fs::read_dir(&blobs_dir).map_or(0, std::iter::Iterator::count);
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn record_last_message_id_sets_field() {
        let (_tmp, rec) = fixture();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.record_last_message_id("msg_abc").await;
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.last_message_id.as_deref(), Some("msg_abc"));
    }

    #[tokio::test]
    async fn record_last_message_id_without_open_prompt_is_noop() {
        let (_tmp, rec) = fixture();
        // No open prompt; must not panic and snapshot stays None.
        rec.record_last_message_id("msg_x").await;
        assert!(rec.snapshot_manifest().await.is_none());
    }

    #[tokio::test]
    async fn mark_partial_sets_flag() {
        let (_tmp, rec) = fixture();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        assert!(!rec.snapshot_manifest().await.unwrap().partial);
        rec.mark_partial().await;
        assert!(rec.snapshot_manifest().await.unwrap().partial);
    }

    #[tokio::test]
    async fn mark_partial_without_open_prompt_is_noop() {
        let (_tmp, rec) = fixture();
        rec.mark_partial().await;
        assert!(rec.snapshot_manifest().await.is_none());
    }

    #[tokio::test]
    async fn close_prompt_without_open_prompt_is_noop() {
        let (_tmp, rec) = fixture();
        // Nothing open — close must succeed and write nothing.
        rec.close_prompt().await.unwrap();
        // Loading a manifest that was never saved is NotFound.
        let err = rec.store.load_manifest(1).unwrap_err();
        assert!(matches!(err, CheckpointError::NotFound(1)));
    }

    #[tokio::test]
    async fn snapshot_manifest_none_when_closed() {
        let (_tmp, rec) = fixture();
        assert!(rec.snapshot_manifest().await.is_none());
    }

    #[tokio::test]
    async fn store_accessor_returns_same_session_dir() {
        let (_tmp, rec) = fixture();
        let dir = rec.store().session_dir().to_path_buf();
        assert!(dir.exists());
        // Two calls return a handle to the same on-disk session dir.
        assert_eq!(rec.store().session_dir(), dir);
    }

    #[tokio::test]
    async fn open_prompt_resets_previous_state() {
        let (tmp, rec) = fixture();
        let path = tmp.path().join("workspace").join("a.txt");
        std::fs::write(&path, "v1").unwrap();
        rec.open_prompt(1, ManifestKind::Files, "first")
            .await
            .unwrap();
        rec.capture(&path, "Write", "tu_1").await.unwrap();
        assert_eq!(rec.snapshot_manifest().await.unwrap().entries.len(), 1);
        // Re-opening replaces the in-memory manifest with a fresh empty one.
        rec.open_prompt(2, ManifestKind::Plan, "second")
            .await
            .unwrap();
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.prompt_index, 2);
        assert!(m.entries.is_empty());
        assert_eq!(m.kind, ManifestKind::Plan);
    }

    #[test]
    fn sha256_hex_is_64_hex_chars() {
        let h = sha256_hex(b"hello");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // Known sha256("hello").
        assert_eq!(
            h,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        // Empty input has the well-known empty digest.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // SKIPPED: a focused test of `max_file_bytes()`'s env-override branch would
    // require mutating `CALIBAN_CHECKPOINT_MAX_FILE_BYTES`. The crate is
    // compiled with `-D unsafe-code`, so `std::env::set_var` (unsafe since Rust
    // 2024) is unavailable, and process-global env mutation would race the
    // parallel test runner regardless. The default path is exercised
    // indirectly by `capture_oversized_file_records_error_and_marks_partial`.

    #[test]
    fn canonicalize_existing_ancestor_appends_nonexistent_tail() {
        let tmp = TempDir::new().unwrap();
        let real = std::fs::canonicalize(tmp.path()).unwrap();
        // The leaf doesn't exist; the ancestor does — function should
        // canonicalise the ancestor and re-append the missing tail.
        let target = tmp.path().join("missing-dir").join("leaf.txt");
        let got = canonicalize_existing_ancestor(&target);
        assert_eq!(got, real.join("missing-dir").join("leaf.txt"));
    }

    #[test]
    fn canonicalize_existing_ancestor_resolves_full_existing_path() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("present.txt");
        std::fs::write(&file, "x").unwrap();
        let got = canonicalize_existing_ancestor(&file);
        assert_eq!(got, std::fs::canonicalize(&file).unwrap());
    }
}
