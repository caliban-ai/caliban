//! Adapter-level error type.

use caliban_provider::Error as ProviderError;

/// Errors produced by the Vertex provider.
#[derive(thiserror::Error, Debug)]
pub enum VertexError {
    /// A required environment-variable or config field was absent.
    #[error("missing config field: {0}")]
    MissingConfig(&'static str),

    /// A config field had an invalid format (e.g. duration string).
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// Failed to obtain a GCP access token.
    #[error("gcp auth error: {0}")]
    Auth(#[source] gcp_auth::Error),

    /// HTTP error talking to a Vertex control-plane endpoint.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// Failed to parse a control-plane JSON response.
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),

    /// A generic transport-level error.
    #[error("transport error: {0}")]
    Transport(Box<dyn std::error::Error + Send + Sync>),
}

impl From<VertexError> for ProviderError {
    fn from(e: VertexError) -> Self {
        Self::adapter(e)
    }
}
