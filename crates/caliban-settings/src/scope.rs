//! Scope enum + canonical-path resolution.
//!
//! Scope precedence (default; from highest to lowest):
//!
//! ```text
//! Cli > Local > Project > User > Managed
//! ```
//!
//! `Managed` flips to the *top* of the chain when its settings contain
//! `parent_settings_behavior: "block"`.

use std::path::{Path, PathBuf};

/// The four real on-disk scopes + the `Cli` virtual overlay injected by
/// `--settings <FILE|JSON>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// `/etc/caliban/managed-settings.json` (Linux) or platform equivalent.
    Managed,
    /// `~/.config/caliban/settings.json` or platform equivalent.
    User,
    /// `<workspace>/.caliban/settings.json`.
    Project,
    /// `<workspace>/.caliban/settings.local.json`.
    Local,
    /// CLI overlay injected by `--settings`.
    Cli,
}

impl Scope {
    /// Short lowercase label suitable for log lines and the `[scope]`
    /// chips in the `/config` overlay.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Cli => "cli",
        }
    }

    /// Parse a scope label from a `--setting-sources` CSV entry.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "managed" => Some(Self::Managed),
            "user" => Some(Self::User),
            "project" => Some(Self::Project),
            "local" => Some(Self::Local),
            "cli" => Some(Self::Cli),
            _ => None,
        }
    }

    /// Canonical `(json, toml)` paths for this scope.
    ///
    /// `Managed` has no path under a project workspace; callers must
    /// pass `home`-style overrides via [`ScopePaths::managed_root`] for
    /// tests.
    #[must_use]
    pub fn canonical_paths(
        self,
        workspace_root: &Path,
        paths: &ScopePaths,
    ) -> Option<(PathBuf, PathBuf)> {
        match self {
            Self::Managed => paths.managed_root.as_ref().map(|root| {
                (
                    root.join("managed-settings.json"),
                    root.join("managed-settings.toml"),
                )
            }),
            Self::User => paths.user_config_dir.as_ref().map(|d| {
                (
                    d.join("caliban").join("settings.json"),
                    d.join("caliban").join("settings.toml"),
                )
            }),
            Self::Project => Some((
                workspace_root.join(".caliban").join("settings.json"),
                workspace_root.join(".caliban").join("settings.toml"),
            )),
            Self::Local => Some((
                workspace_root.join(".caliban").join("settings.local.json"),
                workspace_root.join(".caliban").join("settings.local.toml"),
            )),
            Self::Cli => None,
        }
    }
}

/// Path overrides for scope resolution. Useful for tests that want to
/// substitute fake home/managed directories without touching the real
/// filesystem.
#[derive(Debug, Clone, Default)]
pub struct ScopePaths {
    /// Override for the system-wide `managed-settings` directory.
    /// `/etc/caliban` on Linux by default.
    pub managed_root: Option<PathBuf>,
    /// Override for the user-config directory (the parent that holds
    /// `caliban/settings.json`). Defaults to [`dirs::config_dir`].
    pub user_config_dir: Option<PathBuf>,
}

impl ScopePaths {
    /// Use the production defaults: `/etc/caliban` (Linux managed) and
    /// `dirs::config_dir()` (user).
    #[must_use]
    pub fn defaults() -> Self {
        let managed_root = if cfg!(target_os = "linux") {
            Some(PathBuf::from("/etc/caliban"))
        } else if cfg!(target_os = "macos") {
            Some(PathBuf::from("/Library/Application Support/Caliban"))
        } else if cfg!(target_os = "windows") {
            Some(PathBuf::from("C:/ProgramData/Caliban"))
        } else {
            None
        };
        Self {
            managed_root,
            user_config_dir: dirs::config_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_parse_round_trip() {
        for s in [
            Scope::Managed,
            Scope::User,
            Scope::Project,
            Scope::Local,
            Scope::Cli,
        ] {
            assert_eq!(Scope::parse(s.label()), Some(s));
        }
        assert_eq!(Scope::parse("bogus"), None);
    }

    #[test]
    fn project_paths_resolve() {
        let paths = ScopePaths::default();
        let ws = PathBuf::from("/tmp/ws");
        let (j, t) = Scope::Project.canonical_paths(&ws, &paths).unwrap();
        assert!(j.ends_with(".caliban/settings.json"));
        assert!(t.ends_with(".caliban/settings.toml"));
    }

    #[test]
    fn local_paths_resolve() {
        let paths = ScopePaths::default();
        let ws = PathBuf::from("/tmp/ws");
        let (j, t) = Scope::Local.canonical_paths(&ws, &paths).unwrap();
        assert!(j.ends_with(".caliban/settings.local.json"));
        assert!(t.ends_with(".caliban/settings.local.toml"));
    }
}
