//! Error type for memory-tier loading.

use std::path::PathBuf;

/// Errors that can occur while loading or splicing memory tiers.
#[derive(thiserror::Error, Debug)]
pub enum MemoryError {
    /// IO failure reading a tier file.
    #[error("io error reading {path}: {source}")]
    Io {
        /// The path that failed to read.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Failed to seed or write to the auto-memory directory.
    #[error("failed to initialise auto-memory dir {path}: {source}")]
    AutoMemorySeed {
        /// The directory we tried to create or seed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// Convenience `Result` alias for this crate.
pub type Result<T> = std::result::Result<T, MemoryError>;
