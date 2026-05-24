//! Adapter-level error type.

use caliban_provider::Error as ProviderError;

/// Errors produced by the Bedrock provider.
#[derive(thiserror::Error, Debug)]
pub enum BedrockError {
    /// A required environment-variable or config field was absent.
    #[error("missing config field: {0}")]
    MissingConfig(&'static str),

    /// A config field had an invalid format (e.g. duration string).
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// A generic transport-level error.
    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),
}

impl From<BedrockError> for ProviderError {
    fn from(e: BedrockError) -> Self {
        Self::adapter(e)
    }
}
