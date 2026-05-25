//! Manifest schema (per-prompt) and prompt-summary types.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The flavor of a prompt's manifest.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ManifestKind {
    /// File-touching prompt — `entries` may be non-empty.
    Files,
    /// Plan-mode prompt — `entries` is always empty; emitted as a cursor
    /// marker so `/rewind` can target the prompt for conversation rewind.
    Plan,
    /// Prompt whose blobs were dropped by the bytes-cap sweeper; the
    /// manifest stays as a ⚠-marked marker but `entries` is empty.
    Cleared,
}

/// One file entry in a per-prompt manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Absolute or workspace-relative path that was touched (we store the
    /// canonicalised absolute path so restore is unambiguous).
    pub path: PathBuf,
    /// Hex-encoded sha256 of the pre-image bytes (empty when `exists_pre`
    /// is `false`).
    pub blob_sha256: String,
    /// POSIX file mode (best-effort; `0o644` on Windows).
    #[serde(default = "default_mode")]
    pub mode: u32,
    /// Pre-image size in bytes.
    #[serde(default)]
    pub size: u64,
    /// `true` if the file existed before the prompt touched it. `false`
    /// means restore should *delete* the file (the prompt created it from
    /// scratch).
    pub exists_pre: bool,
    /// Name of the tool that first touched this path within the prompt
    /// (`"Write"`, `"Edit"`, `"MultiEdit"`, `"NotebookEdit"`).
    pub tool_name: String,
    /// Model-issued `tool_use_id` of the first toucher (informational).
    #[serde(default)]
    pub tool_use_id: String,
    /// If pre-image read failed (e.g. unreadable file), the captured error
    /// text. Restore skips entries with `error: Some(_)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn default_mode() -> u32 {
    0o644
}

/// Per-prompt manifest on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Monotonic prompt index within the parent session (1-based for
    /// directory naming; serialised as a plain integer).
    pub prompt_index: u32,
    /// Manifest flavor.
    pub kind: ManifestKind,
    /// Free-form prompt title (first ~80 chars of the user message,
    /// best-effort). Used by the `/rewind` overlay.
    #[serde(default)]
    pub title: String,
    /// When the manifest was created (UTC).
    pub created_at: DateTime<Utc>,
    /// Provider-assigned message id of the last assistant message in this
    /// prompt's turns. Used to truncate the conversation on restore.
    #[serde(default)]
    pub last_message_id: Option<String>,
    /// File entries.
    #[serde(default)]
    pub entries: Vec<ManifestEntry>,
    /// `true` if some blob writes failed and the manifest is incomplete.
    #[serde(default)]
    pub partial: bool,
}

impl Manifest {
    /// Construct an empty manifest for a new prompt.
    #[must_use]
    pub fn new(prompt_index: u32, kind: ManifestKind, title: impl Into<String>) -> Self {
        Self {
            prompt_index,
            kind,
            title: title.into(),
            created_at: Utc::now(),
            last_message_id: None,
            entries: Vec::new(),
            partial: false,
        }
    }
}

/// Compact summary returned by [`crate::CheckpointStore::list_prompts`] —
/// drives the `/rewind` overlay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptSummary {
    /// Prompt index (1-based).
    pub prompt_index: u32,
    /// First ~80 chars of the user message.
    pub title: String,
    /// Manifest flavor.
    pub kind: ManifestKind,
    /// When the manifest was created.
    pub created_at: DateTime<Utc>,
    /// File count (0 for plan-mode prompts).
    pub file_count: usize,
    /// `true` when the manifest carried `partial: true` or `kind == Cleared`.
    pub partial: bool,
}
