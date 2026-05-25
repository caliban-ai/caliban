//! `CheckpointStore` — disk layout + manifest read/write + listing.
//!
//! Layout (matches Claude Code, override via `CALIBAN_CHECKPOINT_ROOT`):
//!
//! ```text
//! <root>/projects/<sanitized-cwd>/checkpoints/<session>/
//!   prompt-001/
//!     manifest.json
//!     blobs/<sha256>.bin
//!   prompt-002/
//!     ...
//! ```

use std::cmp::Reverse;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{CheckpointError, Result};
use crate::manifest::{Manifest, PromptSummary};

/// Encode a byte slice as lowercase hex (no leading `0x`).
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Environment variable that overrides the on-disk root.
pub const ROOT_ENV: &str = "CALIBAN_CHECKPOINT_ROOT";

/// Environment variable that disables recording + pruning.
pub const DISABLED_ENV: &str = "CALIBAN_CHECKPOINT_DISABLED";

/// Resolve the default checkpoint root.
///
/// 1. `$CALIBAN_CHECKPOINT_ROOT` if set (the rebase point for tests).
/// 2. Otherwise `~/.caliban/projects` (so callers can locate it
///    alongside Claude Code's `~/.claude/projects/...` tree).
///
/// # Errors
/// Returns an error when neither env override nor home directory are
/// available — the very rare case of a `tmpfs` chroot with no `HOME`.
pub fn default_root() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var(ROOT_ENV)
        && !custom.is_empty()
    {
        return Ok(PathBuf::from(custom));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        CheckpointError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no home directory",
        ))
    })?;
    Ok(home.join(".caliban").join("projects"))
}

/// Deterministic, filesystem-safe identifier for a workspace cwd.
///
/// Mirrors Claude Code's mapping: `sha256(canonical_path)[..16]` hex-encoded.
/// We canonicalise where possible, fall back to the raw path otherwise.
#[must_use]
pub fn sanitize_cwd(cwd: &Path) -> String {
    let canon = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canon.display().to_string().as_bytes());
    let digest = hasher.finalize();
    let hex = hex_encode(&digest);
    hex[..16].to_string()
}

/// Per-session checkpoint store.
#[derive(Debug, Clone)]
pub struct CheckpointStore {
    /// `<root>/projects/<sanitized-cwd>/checkpoints/<session>/`
    session_dir: PathBuf,
}

impl CheckpointStore {
    /// Open (or create) a store for a `(cwd, session_id)` pair under the
    /// default root.
    ///
    /// # Errors
    /// I/O errors creating the session directory.
    pub fn open(cwd: &Path, session_id: &str) -> Result<Self> {
        Self::open_in(&default_root()?, cwd, session_id)
    }

    /// Open with an explicit root (used by tests).
    ///
    /// # Errors
    /// I/O errors creating the session directory.
    pub fn open_in(root: &Path, cwd: &Path, session_id: &str) -> Result<Self> {
        let sanitized = sanitize_cwd(cwd);
        let session_dir = root
            .join("projects")
            .join(sanitized)
            .join("checkpoints")
            .join(sanitize_session(session_id));
        std::fs::create_dir_all(&session_dir)?;
        Ok(Self { session_dir })
    }

    /// Root path of this store's per-session directory.
    #[must_use]
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Compute the per-prompt directory path (does NOT create it).
    #[must_use]
    pub fn prompt_dir(&self, prompt_index: u32) -> PathBuf {
        self.session_dir.join(format!("prompt-{prompt_index:03}"))
    }

    /// Compute the blob sub-directory path (does NOT create it).
    #[must_use]
    pub fn blobs_dir(&self, prompt_index: u32) -> PathBuf {
        self.prompt_dir(prompt_index).join("blobs")
    }

    /// Path to a content-addressed blob.
    #[must_use]
    pub fn blob_path(&self, prompt_index: u32, sha: &str) -> PathBuf {
        self.blobs_dir(prompt_index).join(format!("{sha}.bin"))
    }

    /// Initialise the per-prompt directory tree (idempotent).
    ///
    /// # Errors
    /// I/O errors.
    pub fn ensure_prompt_dir(&self, prompt_index: u32) -> Result<()> {
        std::fs::create_dir_all(self.blobs_dir(prompt_index))?;
        Ok(())
    }

    /// Persist a manifest atomically (tmpfile + rename).
    ///
    /// # Errors
    /// I/O or serialization errors.
    pub fn save_manifest(&self, manifest: &Manifest) -> Result<()> {
        self.ensure_prompt_dir(manifest.prompt_index)?;
        let path = self.prompt_dir(manifest.prompt_index).join("manifest.json");
        let body = serde_json::to_vec_pretty(manifest)?;
        caliban_common::fs::write_atomic(&path, &body).map_err(CheckpointError::Io)?;
        Ok(())
    }

    /// Load a manifest by prompt index.
    ///
    /// # Errors
    /// `NotFound` when the prompt directory or manifest file doesn't exist;
    /// I/O or serde errors otherwise.
    pub fn load_manifest(&self, prompt_index: u32) -> Result<Manifest> {
        let path = self.prompt_dir(prompt_index).join("manifest.json");
        match std::fs::read(&path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(CheckpointError::NotFound(prompt_index))
            }
            Err(e) => Err(CheckpointError::Io(e)),
        }
    }

    /// Read a blob's bytes by hex sha256.
    ///
    /// # Errors
    /// `BlobMissing` if the file isn't there; I/O otherwise.
    pub fn read_blob(&self, prompt_index: u32, sha: &str) -> Result<Vec<u8>> {
        let path = self.blob_path(prompt_index, sha);
        match std::fs::read(&path) {
            Ok(v) => Ok(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(CheckpointError::BlobMissing {
                    sha: sha.to_string(),
                    path,
                })
            }
            Err(e) => Err(CheckpointError::Io(e)),
        }
    }

    /// Write a blob iff it doesn't already exist (content-addressed dedup).
    ///
    /// Returns `true` when this call performed the write; `false` when the
    /// blob was already present (sha collision-resistant by construction).
    ///
    /// # Errors
    /// I/O errors creating the parent directory or persisting the tempfile.
    pub fn write_blob(&self, prompt_index: u32, sha: &str, bytes: &[u8]) -> Result<bool> {
        let path = self.blob_path(prompt_index, sha);
        if path.exists() {
            return Ok(false);
        }
        caliban_common::fs::write_atomic(&path, bytes).map_err(CheckpointError::Io)?;
        Ok(true)
    }

    /// Enumerate all prompts in this session, newest first.
    ///
    /// Manifests that fail to parse are skipped silently (so a single
    /// corrupted prompt doesn't sink the overlay).
    ///
    /// # Errors
    /// I/O errors reading the session directory (other than `NotFound`).
    pub fn list_prompts(&self) -> Result<Vec<PromptSummary>> {
        let entries = match std::fs::read_dir(&self.session_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CheckpointError::Io(e)),
        };
        let mut summaries: Vec<PromptSummary> = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(idx_str) = file_name.strip_prefix("prompt-") else {
                continue;
            };
            let Ok(idx) = idx_str.parse::<u32>() else {
                continue;
            };
            let Ok(m) = self.load_manifest(idx) else {
                continue;
            };
            let file_count = m.entries.len();
            let kind = m.kind;
            let partial = m.partial || matches!(kind, crate::manifest::ManifestKind::Cleared);
            summaries.push(PromptSummary {
                prompt_index: m.prompt_index,
                title: m.title,
                kind,
                created_at: m.created_at,
                file_count,
                partial,
            });
        }
        // Newest first.
        summaries.sort_by_key(|s| Reverse(s.prompt_index));
        Ok(summaries)
    }

    /// Find the highest existing prompt index in this session. Returns 0
    /// when the session has no checkpoints yet.
    ///
    /// # Errors
    /// I/O errors reading the directory.
    pub fn next_prompt_index(&self) -> Result<u32> {
        let entries = match std::fs::read_dir(&self.session_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(1),
            Err(e) => return Err(CheckpointError::Io(e)),
        };
        let mut max_idx: u32 = 0;
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(rest) = name.strip_prefix("prompt-")
                && let Ok(n) = rest.parse::<u32>()
            {
                max_idx = max_idx.max(n);
            }
        }
        Ok(max_idx + 1)
    }
}

/// Validate / sanitise a session id for safe use as a directory name. Falls
/// back to a sha-derived placeholder for empty or path-traversing inputs.
fn sanitize_session(session_id: &str) -> String {
    let trimmed = session_id.trim();
    if trimmed.is_empty() {
        return "anonymous".to_string();
    }
    let ok = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok && trimmed.len() <= 64 {
        return trimmed.to_string();
    }
    // Fallback: hash the raw id so a path-traversing or oversized session
    // id can't break the directory layout.
    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
    let digest = hasher.finalize();
    let hex = hex_encode(&digest);
    format!("sid-{}", &hex[..16])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, ManifestEntry, ManifestKind};
    use tempfile::TempDir;

    fn store_in(root: &Path) -> CheckpointStore {
        CheckpointStore::open_in(root, Path::new("/cwd/example"), "sess-1").unwrap()
    }

    #[test]
    fn opens_creates_session_dir() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(tmp.path());
        assert!(store.session_dir().exists());
        assert!(store.session_dir().is_dir());
    }

    #[test]
    fn save_and_load_manifest_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(tmp.path());
        let mut m = Manifest::new(1, ManifestKind::Files, "first prompt");
        m.entries.push(ManifestEntry {
            path: PathBuf::from("/tmp/foo.txt"),
            blob_sha256: "deadbeef".into(),
            mode: 0o644,
            size: 5,
            exists_pre: true,
            tool_name: "Edit".into(),
            tool_use_id: "toolu_x".into(),
            error: None,
        });
        store.save_manifest(&m).unwrap();
        let loaded = store.load_manifest(1).unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn list_prompts_newest_first() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(tmp.path());
        for i in 1..=3u32 {
            let m = Manifest::new(i, ManifestKind::Files, format!("p{i}"));
            store.save_manifest(&m).unwrap();
        }
        let prompts = store.list_prompts().unwrap();
        assert_eq!(prompts.len(), 3);
        assert_eq!(prompts[0].prompt_index, 3);
        assert_eq!(prompts[2].prompt_index, 1);
    }

    #[test]
    fn write_blob_dedups_same_content() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(tmp.path());
        let bytes = b"hello world";
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let sha = hex_encode(&hasher.finalize());
        let first = store.write_blob(1, &sha, bytes).unwrap();
        let second = store.write_blob(1, &sha, bytes).unwrap();
        assert!(first);
        assert!(!second);
        assert_eq!(store.read_blob(1, &sha).unwrap().as_slice(), bytes);
    }

    #[test]
    fn next_prompt_index_increments() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(tmp.path());
        assert_eq!(store.next_prompt_index().unwrap(), 1);
        store
            .save_manifest(&Manifest::new(1, ManifestKind::Files, "p1"))
            .unwrap();
        assert_eq!(store.next_prompt_index().unwrap(), 2);
        store
            .save_manifest(&Manifest::new(2, ManifestKind::Plan, "p2"))
            .unwrap();
        assert_eq!(store.next_prompt_index().unwrap(), 3);
    }

    #[test]
    fn sanitize_session_falls_back_on_bad_input() {
        let s = sanitize_session("../etc/passwd");
        assert!(s.starts_with("sid-"));
    }
}
