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
    /// A topic file's frontmatter was missing, malformed, or contained an
    /// invalid `metadata.type` value.
    #[error("invalid topic frontmatter at {path}: {reason}")]
    InvalidTopic {
        /// The topic file path.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },
    /// A topic slug failed validation (path traversal, illegal characters, or empty).
    #[error("invalid topic slug '{slug}': {reason}")]
    InvalidSlug {
        /// The offending slug.
        slug: String,
        /// Reason it was rejected.
        reason: String,
    },
}

/// Convenience `Result` alias for this crate.
pub type Result<T> = std::result::Result<T, MemoryError>;
