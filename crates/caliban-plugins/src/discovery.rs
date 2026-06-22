//! Plugin source discovery — root resolution, fs walk, and the
//! [`PluginSourceProvider`] seam that lets new source kinds (git, HTTP, …) be
//! added without editing the discovery loop or shadowing logic.

use std::path::{Path, PathBuf};

use crate::error::PluginError;
use crate::loaded::{Discovered, PluginSource};

/// A source of plugin candidates. Implementors locate plugin directories
/// (filesystem roots today; git/HTTP later) and report a [`priority`] used to
/// order shadowing — lower priority wins, matching the historical
/// project > user > managed precedence.
///
/// [`priority`]: PluginSourceProvider::priority
pub trait PluginSourceProvider {
    /// Locate every plugin candidate this source exposes.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] only for failures that can't be attributed to a
    /// specific plugin (e.g. an unreadable parent dir).
    fn discover(&self) -> Result<Vec<Discovered>, PluginError>;

    /// Shadowing order — lower wins. Sources are iterated low-to-high so a
    /// candidate from a lower-priority source shadows a same-named candidate
    /// from a higher-priority one.
    fn priority(&self) -> u8;
}

/// A filesystem-backed plugin source: one directory whose immediate
/// subdirectories each (optionally) contain a `plugin.json`.
#[derive(Debug, Clone)]
pub struct DirectorySource {
    root: PathBuf,
    source: PluginSource,
    priority: u8,
}

impl DirectorySource {
    /// Build a directory source rooted at `root`, tagging discovered plugins
    /// with `source` and ordering it at `priority` (lower wins).
    #[must_use]
    pub fn new(root: PathBuf, source: PluginSource, priority: u8) -> Self {
        Self {
            root,
            source,
            priority,
        }
    }
}

impl PluginSourceProvider for DirectorySource {
    fn discover(&self) -> Result<Vec<Discovered>, PluginError> {
        walk_root(&self.root, self.source)
    }

    fn priority(&self) -> u8 {
        self.priority
    }
}

/// Walk a single root directory, yielding one [`Discovered`] per immediate
/// subdirectory that contains a `plugin.json`. A non-existent root yields an
/// empty vec; an existing-but-unreadable root is a hard error (matching the
/// historical `PluginManager::load` behavior).
///
/// # Errors
///
/// Returns [`PluginError::Io`] if `root` exists but cannot be read.
pub fn walk_root(root: &Path, source: PluginSource) -> Result<Vec<Discovered>, PluginError> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    let rd = match std::fs::read_dir(root) {
        Ok(rd) => rd,
        Err(source_err) => {
            return Err(PluginError::Io {
                path: root.to_path_buf(),
                source: source_err,
            });
        }
    };
    for entry in rd.flatten() {
        let plug_dir = entry.path();
        if !plug_dir.is_dir() {
            continue;
        }
        let candidate = Discovered::new(&plug_dir, source);
        if !candidate.manifest_path.exists() {
            continue;
        }
        out.push(candidate);
    }
    Ok(out)
}
