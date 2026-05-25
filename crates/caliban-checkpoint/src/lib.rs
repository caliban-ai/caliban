//! Per-prompt checkpoint + `/rewind` store for the caliban agent harness
//! (ADR 0028).
//!
//! Each `Agent::run` invocation snapshots files that file-writing tools
//! (`Write` / `Edit` / `MultiEdit` / `NotebookEdit`) touched during that
//! prompt's turn(s). The pre-image bytes are content-addressed under
//! `objects/<sha256>`; a per-prompt `manifest.json` lists the file paths +
//! blob hashes. `/rewind` reads the manifest, overwrites the working tree
//! from the blobs, and optionally truncates the message history at the
//! prompt's last message id (or invokes [`SummarizingCompactor`] on a
//! slice).
//!
//! The crate is intentionally I/O-shaped and Hooks-driven — it owns no
//! global state.
//!
//! Bash `rm`/`mv`/`cp` are explicitly NOT tracked (matches Claude Code's
//! documented limitation; non-file-tool mutations are surfaced as a
//! one-time toast).

#![allow(clippy::multiple_crate_versions)]

pub mod error;
pub mod hook;
pub mod manifest;
pub mod prune;
pub mod recorder;
pub mod restore;
pub mod store;

pub use error::{CheckpointError, Result};
pub use hook::CheckpointHook;
pub use manifest::{Manifest, ManifestEntry, ManifestKind, PromptSummary};
pub use recorder::CheckpointRecorder;
pub use restore::{
    ConversationRestoreMode, RestoreOptions, RestoreOutcome, restore, restore_files_only,
};
pub use store::{CheckpointStore, default_root, sanitize_cwd};
