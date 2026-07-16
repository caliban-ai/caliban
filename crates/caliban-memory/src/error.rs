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
    /// A storage-backend error surfaced from a non-fs substrate (e.g. gonzalo).
    #[error("memory backend error: {0}")]
    Backend(String),

    /// An optimistic-concurrency conflict on write/delete. Dormant on the
    /// single-writer fs substrate; live once remote sync exists.
    #[error("memory write conflict at {key}")]
    Conflict {
        /// The topic key involved in the conflict.
        key: String,
    },
}

/// Convenience `Result` alias for this crate.
pub type Result<T> = std::result::Result<T, MemoryError>;

#[cfg(test)]
mod error_variant_tests {
    use super::MemoryError;

    #[test]
    fn conflict_and_backend_variants_display() {
        let c = MemoryError::Conflict {
            key: "caliban/memory:abc/foo".into(),
        };
        assert!(c.to_string().contains("conflict"));
        let b = MemoryError::Backend("boom".into());
        assert!(b.to_string().contains("boom"));
    }
}
