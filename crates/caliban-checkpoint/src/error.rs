//! Error type for the checkpoint store.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Specialised `Result` alias for the checkpoint crate.
pub type Result<T> = std::result::Result<T, CheckpointError>;

/// Errors produced by [`crate::CheckpointStore`] / [`crate::restore::restore`].
#[derive(Debug, Error)]
pub enum CheckpointError {
    /// I/O failure (read/write/rename/remove).
    #[error("checkpoint i/o: {0}")]
    Io(#[from] io::Error),

    /// Serde-json failure on manifest read/write.
    #[error("checkpoint manifest serde: {0}")]
    Serde(#[from] serde_json::Error),

    /// Caller asked for a prompt that doesn't exist.
    #[error("checkpoint not found: prompt-{0:03}")]
    NotFound(u32),

    /// Blob referenced by a manifest could not be located.
    #[error("blob missing: {sha} for {path}")]
    BlobMissing {
        /// Hex-encoded sha256 of the missing blob.
        sha: String,
        /// Path that was meant to be restored from this blob.
        path: PathBuf,
    },

    /// User-facing capture skipped (e.g. non-workspace path). Not fatal —
    /// surfaced for tests and toasts.
    #[error("capture skipped: {reason}")]
    Skipped {
        /// Why the capture was skipped.
        reason: String,
    },

    /// Atomic restore failed mid-way; surfaces the underlying `io::Error` so
    /// callers can show it to operators.
    #[error("atomic restore failed for {path}: {source}")]
    AtomicRestore {
        /// The path that failed to restore.
        path: PathBuf,
        /// Underlying I/O cause.
        source: io::Error,
    },

    /// A blob read back at restore time did not match the manifest's
    /// content-address (sha256 / size) — a corrupt or truncated blob. The
    /// live file is left untouched rather than overwritten with bad data
    /// (#220 issue 5).
    #[error(
        "blob integrity check failed for {path}: expected sha {expected_sha} ({expected_size} bytes), \
         got {actual_sha} ({actual_size} bytes)"
    )]
    BlobIntegrity {
        /// Path that would have been restored.
        path: PathBuf,
        /// sha256 recorded in the manifest.
        expected_sha: String,
        /// sha256 of the blob bytes actually read back.
        actual_sha: String,
        /// Size recorded in the manifest.
        expected_size: u64,
        /// Size of the blob bytes actually read back.
        actual_size: u64,
    },
}
