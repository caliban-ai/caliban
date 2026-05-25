//! `LoadedPlugin` and friends — the in-memory representation handed to the
//! caliban binary after discovery.

use std::path::PathBuf;

use crate::manifest::{PluginManifest, ResolvedComponents};

/// Where a plugin was discovered. Determines override semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginSource {
    /// `<workspace>/.caliban/plugins/<name>/`
    Project,
    /// `$XDG_DATA_HOME/caliban/plugins/<name>/`
    User,
    /// `/etc/caliban/plugins/<name>/` (Linux), platform analogues elsewhere.
    Managed,
}

impl PluginSource {
    /// Stable lower-case label for logs / overlays.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
            Self::Managed => "managed",
        }
    }
}

/// A namespaced item produced by a plugin (`pluginA:skill-foo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespacedItem {
    /// `<plugin>:<bare-name>` (e.g. `superpowers:brainstorming`).
    pub namespaced: String,
    /// The bare item name (e.g. `brainstorming`).
    pub bare: String,
    /// The plugin namespace (e.g. `superpowers`).
    pub plugin: String,
}

impl NamespacedItem {
    /// Construct from a plugin name + bare item name.
    #[must_use]
    pub fn new(plugin: &str, bare: &str) -> Self {
        Self {
            namespaced: format!("{plugin}:{bare}"),
            bare: bare.to_string(),
            plugin: plugin.to_string(),
        }
    }
}

/// A plugin that successfully passed manifest validation and platform/version
/// gating. `components` is already resolved to absolute paths under `root_dir`.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// Parsed manifest.
    pub manifest: PluginManifest,
    /// Absolute path to the plugin directory.
    pub root_dir: PathBuf,
    /// Namespace string — always == `manifest.name`. Kept separately for
    /// quick access by overlays.
    pub namespace: String,
    /// Source root the plugin was discovered in.
    pub source: PluginSource,
    /// Component paths resolved against `root_dir`.
    pub components: ResolvedComponents,
}

impl LoadedPlugin {
    /// Compose `<namespace>:<bare>` for a discovered item.
    #[must_use]
    pub fn namespace_item(&self, bare: &str) -> String {
        format!("{}:{bare}", self.namespace)
    }
}
