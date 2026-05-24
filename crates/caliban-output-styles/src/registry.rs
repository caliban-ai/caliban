//! `OutputStylesRegistry` — owns the loaded styles and exposes name-based
//! lookup for the binary, the TUI overlay, and the system-prompt splice.

use std::path::Path;

use crate::loader::{DiscoveryRoots, default_roots, load_styles, select_active};
use crate::style::OutputStyle;

/// Holds all output styles discovered at startup.
///
/// Construct via [`OutputStylesRegistry::load`] (workspace-aware) or
/// [`OutputStylesRegistry::from_styles`] (in tests, with a hand-rolled
/// list). The four built-in styles are always present unless shadowed by
/// a higher-priority source.
#[derive(Debug, Clone)]
pub struct OutputStylesRegistry {
    styles: Vec<OutputStyle>,
}

impl OutputStylesRegistry {
    /// Load styles from the default discovery roots rooted at
    /// `workspace_root`.
    #[must_use]
    pub fn load(workspace_root: &Path) -> Self {
        Self::load_from(&default_roots(workspace_root))
    }

    /// Load styles from a caller-provided set of discovery roots.
    #[must_use]
    pub fn load_from(roots: &DiscoveryRoots) -> Self {
        Self {
            styles: load_styles(roots),
        }
    }

    /// Construct a registry directly from a list of styles. Used by tests
    /// and (eventually) by plugin reloading paths.
    #[must_use]
    pub fn from_styles(styles: Vec<OutputStyle>) -> Self {
        Self { styles }
    }

    /// Return the style with the given name (or namespaced
    /// `<plugin>:<name>`), if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&OutputStyle> {
        self.styles.iter().find(|s| s.name == name)
    }

    /// Return all available styles, sorted by name.
    #[must_use]
    pub fn available(&self) -> Vec<&OutputStyle> {
        self.styles.iter().collect()
    }

    /// Select the active style for `requested`, honoring the
    /// `force_for_plugin` override when any of `enabled_plugins` ship a
    /// pinned style.
    ///
    /// Falls back to the built-in `default` style (and logs a warning) when
    /// `requested` does not match any loaded style.
    #[must_use]
    pub fn select(&self, requested: &str, enabled_plugins: &[String]) -> Option<OutputStyle> {
        select_active(&self.styles, requested, enabled_plugins)
    }

    /// Number of loaded styles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    /// `true` when no styles loaded (should be impossible in practice
    /// because the four built-ins are always present).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }
}
