//! Error type for caliban-agent-core.

/// Top-level error type for `caliban-agent-core`.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// A provider call failed.
    #[error("provider error: {0}")]
    Provider(#[from] caliban_provider::Error),

    /// A tool's `invoke` returned an error.
    #[error("tool '{tool}' execution failed: {source}")]
    ToolExecution {
        /// The name of the tool that failed.
        tool: String,
        /// The underlying error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The compaction step failed.
    #[error("compaction failed: {0}")]
    Compaction(String),

    /// A hook returned an error.
    #[error("hook failed: {0}")]
    HookFailed(String),

    /// The agent reached `max_turns` without a natural stop.
    #[error("max turns reached ({0}); the model did not naturally stop")]
    MaxTurnsReached(u32),

    /// The operation was cancelled via a `CancellationToken`.
    #[error("operation cancelled")]
    Cancelled,

    /// The agent was built with an invalid or missing configuration field.
    #[error("agent misconfigured: {0}")]
    Misconfigured(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
