//! Errors for session persistence.

/// Errors that can occur during session persistence operations.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// An underlying I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON serialization or deserialization error occurred.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// The session name failed validation (must match `[a-zA-Z0-9_-]+`, length 1..=64).
    #[error("invalid session name '{0}': must match [a-zA-Z0-9_-]+ and be 1..=64 chars")]
    InvalidName(String),
    /// The user's home directory could not be determined.
    #[error("home directory not found")]
    NoHome,
}

/// Convenience alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
