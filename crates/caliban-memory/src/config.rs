//! `MemoryConfig` — paths, dirs, and token budget for tier loading.

use std::path::{Path, PathBuf};

use crate::sanitize::sanitize_workspace;

const DEFAULT_BUDGET_TOKENS: usize = 8_000;

/// Resolved configuration for one memory-load invocation.
#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Path to the operator-global `CLAUDE.md`. `None` if none was discoverable.
    pub global_path: Option<PathBuf>,
    /// Path to the project `CLAUDE.md` (at the workspace root).
    pub project_path: Option<PathBuf>,
    /// Per-workspace auto-memory directory. Always set; may not exist yet.
    pub auto_memory_dir: PathBuf,
    /// Approximate token budget for the combined memory prefix.
    pub max_tokens: usize,
}

impl MemoryConfig {
    /// Resolve a `MemoryConfig` from the environment + the given workspace root.
    ///
    /// Env vars honored:
    /// - `XDG_CONFIG_HOME` (default `~/.config`) → global `CLAUDE.md` lives here.
    /// - `XDG_DATA_HOME` (default `~/.local/share`) → auto-memory base.
    /// - `CALIBAN_MEMORY_DIR` overrides the auto-memory directory root
    ///   (useful for tests + isolated installs).
    /// - `CALIBAN_AUTO_MEMORY_DIRECTORY` overrides the *full* per-project
    ///   auto-memory directory (skips workspace-sanitization + the
    ///   `<root>/<slug>/memory` join). Takes precedence over `CALIBAN_MEMORY_DIR`.
    /// - `CALIBAN_MEMORY_BUDGET_TOKENS` overrides the default `8_000` budget.
    #[must_use]
    pub fn from_env(workspace_root: &Path) -> Self {
        let config_home = xdg_dir("XDG_CONFIG_HOME", dirs::config_dir);
        let data_home = xdg_dir("XDG_DATA_HOME", dirs::data_local_dir);

        let global_path = config_home.map(|d| d.join("caliban").join("CLAUDE.md"));
        let project_path = Some(workspace_root.join("CLAUDE.md"));

        // Full-directory override wins.
        let auto_memory_dir = if let Some(dir) = std::env::var_os("CALIBAN_AUTO_MEMORY_DIRECTORY") {
            PathBuf::from(dir)
        } else {
            let auto_memory_root = std::env::var_os("CALIBAN_MEMORY_DIR")
                .map(PathBuf::from)
                .or_else(|| data_home.map(|d| d.join("caliban").join("projects")));
            let slug = sanitize_workspace(workspace_root);
            auto_memory_root
                .unwrap_or_else(|| PathBuf::from("./.caliban/projects"))
                .join(slug)
                .join("memory")
        };

        let max_tokens = std::env::var("CALIBAN_MEMORY_BUDGET_TOKENS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_BUDGET_TOKENS);

        Self {
            global_path,
            project_path,
            auto_memory_dir,
            max_tokens,
        }
    }
}

/// Resolve an XDG directory: honor the env var if set + non-empty, else fall
/// back to the `dirs` crate's platform default.
fn xdg_dir(env_var: &str, fallback: fn() -> Option<PathBuf>) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(env_var)
        && !v.is_empty()
    {
        return Some(PathBuf::from(v));
    }
    fallback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budget_constant_matches() {
        assert_eq!(DEFAULT_BUDGET_TOKENS, 8_000);
    }

    #[test]
    fn project_path_joins_workspace_root() {
        let cfg = MemoryConfig::from_env(Path::new("/tmp/my-workspace"));
        assert_eq!(
            cfg.project_path.as_deref(),
            Some(Path::new("/tmp/my-workspace/CLAUDE.md"))
        );
    }
}
