//! Error type for the plugin orchestrator.

use std::path::PathBuf;

use thiserror::Error;

/// Errors emitted by the plugin orchestrator.
#[derive(Debug, Error)]
pub enum PluginError {
    /// IO failure with a contextualized path.
    #[error("plugin io {path}: {source}")]
    Io {
        /// Path that triggered the failure.
        path: PathBuf,
        /// Underlying io error.
        #[source]
        source: std::io::Error,
    },

    /// JSON parse failure with a contextualized path.
    #[error("plugin parse {path}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },

    /// Manifest validation failure (e.g. invalid name, bad `min_version`).
    #[error("plugin invalid {path}: {message}")]
    Invalid {
        /// Path of the manifest.
        path: PathBuf,
        /// Human-readable problem description.
        message: String,
    },

    /// Plugin `name` doesn't match the parent directory.
    #[error(
        "plugin name '{manifest_name}' does not match parent directory '{dir_name}' (at {path})"
    )]
    NameMismatch {
        /// Name field in `plugin.json`.
        manifest_name: String,
        /// On-disk directory name.
        dir_name: String,
        /// Path of the manifest.
        path: PathBuf,
    },

    /// Marketplace install rejected: marketplace not in the strict allowlist.
    #[error("marketplace '{url}' is not in plugins.marketplaces.strict_known")]
    UnknownMarketplace {
        /// Offending URL.
        url: String,
    },

    /// Marketplace install rejected: marketplace is in the block list.
    #[error("marketplace '{url}' is in plugins.marketplaces.blocked")]
    BlockedMarketplace {
        /// Offending URL.
        url: String,
    },

    /// Plugin not found in the marketplace index.
    #[error("plugin '{name}' not found in marketplace '{url}'")]
    PluginNotFound {
        /// Plugin name requested.
        name: String,
        /// Marketplace URL.
        url: String,
    },

    /// Sha256 mismatch between marketplace metadata and the downloaded tarball.
    #[error("sha256 mismatch for '{name}' (expected {expected}, got {actual})")]
    Sha256Mismatch {
        /// Plugin name.
        name: String,
        /// Expected hex digest.
        expected: String,
        /// Computed hex digest.
        actual: String,
    },

    /// Plugin disabled by `CALIBAN_STRICT_PLUGIN_ONLY_CUSTOMIZATION` policy.
    #[error(
        "plugin '{name}' is not managed-scope; strict-plugin-only-customization mode rejects it"
    )]
    StrictPluginOnly {
        /// Plugin name.
        name: String,
    },

    /// HTTP transport failure (marketplace fetch, tarball download).
    #[error("http transport error: {0}")]
    Http(#[from] reqwest::Error),

    /// Url parse failure.
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    /// Tarball extraction failure.
    #[error("extract: {0}")]
    Extract(String),
}
