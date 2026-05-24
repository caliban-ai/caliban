//! Errors emitted by the MCP client crate.

use std::path::PathBuf;

/// Errors emitted by the MCP client crate.
#[derive(thiserror::Error, Debug)]
pub enum McpError {
    /// IO failure reading a config file (other than `NotFound`, which is
    /// silently treated as "no config").
    #[error("mcp: io error reading {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse error.
    #[error("mcp: config parse error in {path}: {source}")]
    ConfigParse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
    /// Server key doesn't match `[a-z0-9_-]{1,32}`.
    #[error("mcp: invalid server name '{0}' (must match [a-z0-9_-]{{1,32}})")]
    InvalidServerName(String),
    /// `${VAR}` substitution found no value in the process env.
    #[error("mcp: env var '{var}' referenced by server '{server}' is not set")]
    MissingEnv {
        /// Server whose env table referenced the missing variable.
        server: String,
        /// Variable name that was missing.
        var: String,
    },
    /// `${VAR}` was used inline (e.g. `"prefix-${VAR}-suffix"`). v1 only
    /// supports full-value substitution.
    #[error(
        "mcp: server '{server}' env['{key}'] uses unsupported inline interpolation; only \"${{VAR}}\" full-value substitution is allowed in v1"
    )]
    InlineInterpolation {
        /// Server whose env value was malformed.
        server: String,
        /// Env-table key whose value was malformed.
        key: String,
    },
    /// Spawning a server's command failed (v2: real-runtime errors).
    #[error("mcp: server '{server}' failed to spawn: {source}")]
    Spawn {
        /// Server that failed.
        server: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
}

/// Result alias scoped to this crate.
pub type Result<T> = std::result::Result<T, McpError>;
