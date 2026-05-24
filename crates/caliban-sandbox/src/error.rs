//! Error type for the sandbox layer.

use std::path::PathBuf;

use thiserror::Error;

/// All failure modes of the sandbox layer.
#[derive(Debug, Error)]
pub enum SandboxError {
    /// The configured backend binary (`sandbox-exec` / `bwrap`) was not
    /// found at `looked_at`.
    #[error("sandbox backend '{backend}' not available at {looked_at}")]
    BackendUnavailable {
        /// Backend name (`"sandbox-exec"` or `"bwrap"`).
        backend: &'static str,
        /// Path that was probed.
        looked_at: PathBuf,
    },

    /// The backend binary exists but reports a version older than what
    /// we require.
    #[error("sandbox backend '{backend}' too old: found {found}, need >= {need}")]
    BackendTooOld {
        /// Backend name.
        backend: &'static str,
        /// Version string reported by the binary.
        found: String,
        /// Minimum required version.
        need: String,
    },

    /// Failed to write the generated Seatbelt profile to disk.
    #[error("failed to write sandbox profile to {path}: {source}")]
    PolicyWrite {
        /// Path we attempted to write.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The policy is internally inconsistent or references unusable paths.
    #[error("invalid sandbox config: {reason}")]
    InvalidConfig {
        /// Human-readable reason for the rejection.
        reason: String,
    },

    /// The host platform does not support the sandbox layer in v1
    /// (currently: Windows native).
    #[error("sandbox not supported on platform '{os}'")]
    UnsupportedPlatform {
        /// `std::env::consts::OS` of the host.
        os: &'static str,
    },
}
