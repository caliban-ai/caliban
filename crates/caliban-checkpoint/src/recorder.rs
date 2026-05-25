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
}
