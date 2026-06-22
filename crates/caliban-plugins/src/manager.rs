//! Plugin discovery + filter + namespacing — a thin orchestrating facade.
//!
//! The heavy lifting lives in three sibling modules:
//! - [`crate::discovery`] — root resolution, fs walk, and the
//!   [`PluginSourceProvider`](crate::discovery::PluginSourceProvider) seam.
//! - [`crate::filter`] — platform / version / strict / enable-list gating.
//! - [`crate::aggregate`] — component roots + hooks/MCP aggregation + semver
//!   padding.
//!
//! [`PluginManager::load`] wires them together: build a priority-ordered list
//! of [`PluginSourceProvider`](crate::discovery::PluginSourceProvider)s, walk
//! each, filter every candidate, and dedup by name (lower priority shadows).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::aggregate;
use crate::discovery::{DirectorySource, PluginSourceProvider};
use crate::error::PluginError;
use crate::filter;
use crate::loaded::{LoadedPlugin, PluginSource};

/// Discovery roots, in priority order (project beats user beats managed).
#[derive(Debug, Clone)]
pub struct PluginRoots {
    /// `<workspace>/.caliban/plugins/`
    pub project: Option<PathBuf>,
    /// `$XDG_DATA_HOME/caliban/plugins/`
    pub user: Option<PathBuf>,
    /// `/etc/caliban/plugins/` (Linux), `/Library/Application Support/Caliban/plugins/` (macOS).
    pub managed: Option<PathBuf>,
}

impl PluginRoots {
    /// Default roots derived from the workspace + XDG dirs + OS-specific
    /// managed location.
    #[must_use]
    pub fn default_for(workspace_root: &Path) -> Self {
        let project = Some(workspace_root.join(".caliban").join("plugins"));
        let user =
            caliban_common::paths::platform_data_dir().map(|d| d.join("caliban").join("plugins"));
        let managed = Some(default_managed_dir());
        Self {
            project,
            user,
            managed,
        }
    }

    /// Iterate over `(root, source)` pairs in priority order.
    #[must_use]
    pub fn ordered(&self) -> Vec<(PathBuf, PluginSource)> {
        let mut out = Vec::with_capacity(3);
        if let Some(p) = &self.project {
            out.push((p.clone(), PluginSource::Project));
        }
        if let Some(p) = &self.user {
            out.push((p.clone(), PluginSource::User));
        }
        if let Some(p) = &self.managed {
            out.push((p.clone(), PluginSource::Managed));
        }
        out
    }

    /// Build the priority-ordered list of plugin sources backing these roots.
    /// Each configured root becomes a [`DirectorySource`]; priorities follow
    /// the historical project (0) > user (1) > managed (2) precedence so a
    /// lower-priority source shadows same-named plugins from higher ones.
    ///
    /// Adding a git/HTTP source later means pushing another
    /// [`PluginSourceProvider`] here — no edits to the discovery loop or the
    /// shadowing logic.
    #[must_use]
    pub fn sources(&self) -> Vec<Box<dyn PluginSourceProvider>> {
        let mut out: Vec<Box<dyn PluginSourceProvider>> = Vec::new();
        if let Some(p) = &self.project {
            out.push(Box::new(DirectorySource::new(
                p.clone(),
                PluginSource::Project,
                0,
            )));
        }
        if let Some(p) = &self.user {
            out.push(Box::new(DirectorySource::new(
                p.clone(),
                PluginSource::User,
                1,
            )));
        }
        if let Some(p) = &self.managed {
            out.push(Box::new(DirectorySource::new(
                p.clone(),
                PluginSource::Managed,
                2,
            )));
        }
        out
    }
}

/// Default OS-managed plugin root.
#[must_use]
pub fn default_managed_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/Caliban/plugins")
    }
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/etc/caliban/plugins")
    }
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"C:\ProgramData\Caliban\plugins")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        PathBuf::from("/etc/caliban/plugins")
    }
}

/// Operator-visible settings read by the manager. The settings.json keys
/// (ADR 0026) land later; for now these are read from env vars in the
/// binary and passed in.
#[derive(Debug, Clone, Default)]
pub struct PluginSettings {
    /// When `Some`, only listed plugins are loaded. When `None`, all
    /// discovered plugins are loaded. Managed plugins ignore this filter.
    pub enabled: Option<Vec<String>>,
    /// When true, non-managed plugins (project + user) are rejected.
    /// Matches Claude Code's `strictPluginOnlyCustomization`.
    pub strict_plugin_only_customization: bool,
    /// Running caliban version (used for `caliban.min_version` checks).
    pub caliban_version: Option<String>,
}

impl PluginSettings {
    /// Build a settings struct from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let enabled = std::env::var("CALIBAN_ENABLED_PLUGINS").ok().map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        });
        let strict = matches!(
            std::env::var("CALIBAN_STRICT_PLUGIN_ONLY_CUSTOMIZATION")
                .ok()
                .as_deref(),
            Some("1" | "true" | "TRUE" | "True" | "yes")
        );
        let caliban_version = option_env!("CARGO_PKG_VERSION").map(str::to_string);
        Self {
            enabled,
            strict_plugin_only_customization: strict,
            caliban_version,
        }
    }
}

/// Discovery + filter result.
#[derive(Debug, Default, Clone)]
pub struct PluginManager {
    plugins: Vec<LoadedPlugin>,
    /// Per-plugin parse / validation errors, surfaced in `/plugins`.
    failures: Vec<PluginLoadFailure>,
}

/// A plugin that *was* discovered on disk but failed manifest validation.
#[derive(Debug, Clone)]
pub struct PluginLoadFailure {
    /// Absolute path of the plugin dir.
    pub root_dir: PathBuf,
    /// Source root.
    pub source: PluginSource,
    /// Best-effort directory name (used as a stand-in for `name`).
    pub dir_name: String,
    /// Human-readable error.
    pub error: String,
}

impl PluginManager {
    /// Load every plugin discoverable under `roots`, applying `settings`
    /// filters. The returned manager is safe to share read-only.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] only for failures that can't be attributed
    /// to a specific plugin (e.g. an unreadable parent dir). Per-plugin
    /// errors are recorded in [`Self::failures`] and surfaced in the
    /// `/plugins` overlay.
    pub fn load(roots: &PluginRoots, settings: &PluginSettings) -> Result<Self, PluginError> {
        let mut by_name: BTreeMap<String, LoadedPlugin> = BTreeMap::new();
        let mut failures: Vec<PluginLoadFailure> = Vec::new();

        // Sources iterated in priority order (lower wins): the earlier a
        // candidate is loaded, the higher-priority root it came from, so a
        // later same-named candidate is shadowed.
        let mut sources = roots.sources();
        sources.sort_by_key(|s| s.priority());

        for src in &sources {
            for cand in src.discover()? {
                match filter::try_load_one(
                    &cand.plug_dir,
                    &cand.manifest_path,
                    cand.source,
                    settings,
                ) {
                    Ok(Some(p)) => {
                        if let Some(existing) = by_name.get(&p.manifest.name) {
                            tracing::debug!(
                                target: caliban_common::tracing_targets::TARGET_PLUGINS,
                                name = %p.manifest.name,
                                shadowed_by = %existing.source.as_str(),
                                source = %p.source.as_str(),
                                "skipping shadowed plugin (already loaded from higher-priority root)",
                            );
                        } else {
                            by_name.insert(p.manifest.name.clone(), p);
                        }
                    }
                    Ok(None) => {
                        // Filtered out (disabled, platform mismatch, etc.)
                    }
                    Err(e) => {
                        failures.push(PluginLoadFailure {
                            root_dir: cand.plug_dir.clone(),
                            source: cand.source,
                            dir_name: cand.dir_name.clone(),
                            error: e.to_string(),
                        });
                    }
                }
            }
        }

        Ok(Self {
            plugins: by_name.into_values().collect(),
            failures,
        })
    }

    /// Loaded plugins, ordered alphabetically by name.
    #[must_use]
    pub fn loaded(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    /// Per-plugin failures (for `/plugins` overlay).
    #[must_use]
    pub fn failures(&self) -> &[PluginLoadFailure] {
        &self.failures
    }

    /// Return the union of skill discovery roots. When the manifest's
    /// `components.skills` is set, the returned paths are the explicit
    /// subdirectories. When unset, falls back to `<plugin>/skills/`.
    #[must_use]
    pub fn skill_roots(&self) -> Vec<PathBuf> {
        aggregate::skill_roots(&self.plugins)
    }

    /// Same as [`skill_roots`](Self::skill_roots) for output styles. Returned
    /// paths are *directories* containing `.md` files; if the manifest
    /// enumerated individual files, those file paths are returned as-is.
    #[must_use]
    pub fn output_style_roots(&self) -> Vec<PathBuf> {
        aggregate::output_style_roots(&self.plugins)
    }

    /// Same as [`skill_roots`](Self::skill_roots) for agents.
    #[must_use]
    pub fn agent_roots(&self) -> Vec<PathBuf> {
        aggregate::agent_roots(&self.plugins)
    }

    /// Merged hooks config across all loaded plugins. Each plugin's
    /// hooks file is read, `${CALIBAN_PLUGIN_ROOT}` expanded, and the
    /// resulting `serde_json::Value` returned in load order. The downstream
    /// hooks loader is responsible for merging into its TOML world.
    #[must_use]
    pub fn hooks_configs(&self) -> Vec<(String, serde_json::Value)> {
        aggregate::hooks_configs(&self.plugins)
    }

    /// Merged MCP server configs across plugins. Inline `mcpServers` block
    /// wins over `components.mcp_servers` when both are present (with a
    /// warning). Each server name is namespaced `<plugin>:<server>`.
    #[must_use]
    pub fn mcp_servers(&self) -> Vec<(String, serde_json::Value)> {
        aggregate::mcp_servers(&self.plugins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_plugin(root: &Path, name: &str, body: &str) {
        let plug_dir = root.join(name);
        fs::create_dir_all(&plug_dir).unwrap();
        fs::write(plug_dir.join("plugin.json"), body).unwrap();
    }

    fn minimal(name: &str) -> String {
        format!(r#"{{ "name": "{name}", "version": "0.1.0", "description": "x" }}"#)
    }

    #[test]
    fn discovers_project_plugin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = tmp.path().join(".caliban").join("plugins");
        fs::create_dir_all(&project_root).unwrap();
        make_plugin(&project_root, "demo", &minimal("demo"));
        let roots = PluginRoots {
            project: Some(project_root),
            user: None,
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert_eq!(mgr.loaded().len(), 1);
        assert_eq!(mgr.loaded()[0].source, PluginSource::Project);
        assert_eq!(mgr.loaded()[0].namespace, "demo");
    }

    #[test]
    fn project_shadows_user_with_same_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let user = tmp.path().join("user");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&user).unwrap();
        make_plugin(&project, "demo", &minimal("demo"));
        make_plugin(&user, "demo", &minimal("demo"));
        let roots = PluginRoots {
            project: Some(project),
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert_eq!(mgr.loaded().len(), 1);
        assert_eq!(mgr.loaded()[0].source, PluginSource::Project);
    }

    #[test]
    fn managed_root_loads_too() {
        let tmp = tempfile::TempDir::new().unwrap();
        let managed = tmp.path().join("managed");
        fs::create_dir_all(&managed).unwrap();
        make_plugin(&managed, "policy", &minimal("policy"));
        let roots = PluginRoots {
            project: None,
            user: None,
            managed: Some(managed),
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert_eq!(mgr.loaded().len(), 1);
        assert_eq!(mgr.loaded()[0].source, PluginSource::Managed);
    }

    #[test]
    fn enabled_filter_excludes_user_plugin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        make_plugin(&user, "demo", &minimal("demo"));
        make_plugin(&user, "other", &minimal("other"));
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let settings = PluginSettings {
            enabled: Some(vec!["demo".to_string()]),
            ..Default::default()
        };
        let mgr = PluginManager::load(&roots, &settings).unwrap();
        assert_eq!(mgr.loaded().len(), 1);
        assert_eq!(mgr.loaded()[0].namespace, "demo");
    }

    #[test]
    fn managed_ignores_enabled_filter() {
        let tmp = tempfile::TempDir::new().unwrap();
        let managed = tmp.path().join("managed");
        fs::create_dir_all(&managed).unwrap();
        make_plugin(&managed, "policy", &minimal("policy"));
        let roots = PluginRoots {
            project: None,
            user: None,
            managed: Some(managed),
        };
        let settings = PluginSettings {
            enabled: Some(vec!["something-else".to_string()]),
            ..Default::default()
        };
        let mgr = PluginManager::load(&roots, &settings).unwrap();
        assert_eq!(mgr.loaded().len(), 1);
    }

    #[test]
    fn strict_plugin_only_rejects_user_scope() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let managed = tmp.path().join("managed");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(&managed).unwrap();
        make_plugin(&user, "demo", &minimal("demo"));
        make_plugin(&managed, "policy", &minimal("policy"));
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: Some(managed),
        };
        let settings = PluginSettings {
            strict_plugin_only_customization: true,
            ..Default::default()
        };
        let mgr = PluginManager::load(&roots, &settings).unwrap();
        // Only the managed plugin loads; user-scoped becomes a failure record.
        assert_eq!(mgr.loaded().len(), 1);
        assert_eq!(mgr.loaded()[0].namespace, "policy");
        assert_eq!(mgr.failures().len(), 1);
        assert!(mgr.failures()[0].error.contains("strict"));
    }

    #[test]
    fn malformed_manifest_recorded_as_failure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        make_plugin(&user, "demo", "{ not json");
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert!(mgr.loaded().is_empty());
        assert_eq!(mgr.failures().len(), 1);
        assert_eq!(mgr.failures()[0].dir_name, "demo");
    }

    #[test]
    fn skill_roots_returns_plugin_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        make_plugin(&user, "demo", &minimal("demo"));
        let roots = PluginRoots {
            project: None,
            user: Some(user.clone()),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let sr = mgr.skill_roots();
        assert_eq!(sr, vec![user.join("demo").join("skills")]);
    }

    #[test]
    fn hooks_config_expands_caliban_plugin_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        let plug_dir = user.join("demo");
        fs::create_dir_all(plug_dir.join("hooks")).unwrap();
        fs::write(plug_dir.join("plugin.json"), minimal("demo")).unwrap();
        fs::write(
            plug_dir.join("hooks").join("hooks.json"),
            r#"{ "PreToolUse": [{ "command": "${CALIBAN_PLUGIN_ROOT}/bin/run" }] }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let hc = mgr.hooks_configs();
        assert_eq!(hc.len(), 1);
        let val = &hc[0].1;
        let cmd = val["PreToolUse"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("/demo/bin/run"));
        assert!(!cmd.contains("${"));
    }

    #[test]
    fn hooks_config_honors_claude_plugin_root_alias() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(plug_dir.join("hooks")).unwrap();
        fs::write(plug_dir.join("plugin.json"), minimal("demo")).unwrap();
        fs::write(
            plug_dir.join("hooks").join("hooks.json"),
            r#"{ "PreToolUse": [{ "command": "${CLAUDE_PLUGIN_ROOT}/bin/run" }] }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let hc = mgr.hooks_configs();
        let cmd = hc[0].1["PreToolUse"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("/demo/bin/run"));
    }

    #[test]
    fn mcp_inline_namespaces_servers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(&plug_dir).unwrap();
        let raw = r#"{
            "name": "demo", "version": "0.1.0",
            "mcpServers": {
                "fix": { "command": "${CALIBAN_PLUGIN_ROOT}/bin/fix" }
            }
        }"#;
        fs::write(plug_dir.join("plugin.json"), raw).unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let servers = mgr.mcp_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "demo:fix");
        let cmd = servers[0].1["command"].as_str().unwrap();
        assert!(cmd.ends_with("/demo/bin/fix"));
    }

    #[test]
    fn min_version_too_old_skips_plugin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        let plug_dir = user.join("demo");
        fs::create_dir_all(&plug_dir).unwrap();
        fs::write(
            plug_dir.join("plugin.json"),
            r#"{ "name": "demo", "version": "0.1.0", "caliban": { "min_version": "99.0.0" } }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let settings = PluginSettings {
            caliban_version: Some("0.5.0".into()),
            ..Default::default()
        };
        let mgr = PluginManager::load(&roots, &settings).unwrap();
        assert!(mgr.loaded().is_empty());
    }

    #[test]
    fn min_version_satisfied_loads_plugin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(&plug_dir).unwrap();
        // Partial "0.5" min_version is padded to "0.5.0"; current 1.0 >= 0.5.
        fs::write(
            plug_dir.join("plugin.json"),
            r#"{ "name": "demo", "version": "0.1.0", "caliban": { "min_version": "0.5" } }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let settings = PluginSettings {
            caliban_version: Some("1.0".into()),
            ..Default::default()
        };
        let mgr = PluginManager::load(&roots, &settings).unwrap();
        assert_eq!(mgr.loaded().len(), 1);
    }

    #[test]
    fn name_mismatch_recorded_as_failure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("wrongdir");
        fs::create_dir_all(&plug_dir).unwrap();
        // Manifest name "demo" does not match dir "wrongdir".
        fs::write(plug_dir.join("plugin.json"), minimal("demo")).unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert!(mgr.loaded().is_empty());
        assert_eq!(mgr.failures().len(), 1);
        assert_eq!(mgr.failures()[0].dir_name, "wrongdir");
        assert_eq!(mgr.failures()[0].source, PluginSource::User);
    }

    #[test]
    fn dir_without_manifest_is_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        // Directory with no plugin.json is skipped silently.
        fs::create_dir_all(user.join("not-a-plugin")).unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert!(mgr.loaded().is_empty());
        assert!(mgr.failures().is_empty());
    }

    #[test]
    fn nonexistent_root_is_skipped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let roots = PluginRoots {
            project: Some(tmp.path().join("does-not-exist")),
            user: None,
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert!(mgr.loaded().is_empty());
    }

    #[test]
    fn file_entry_in_root_is_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        // A plain file (not a dir) at the root level is skipped.
        fs::write(user.join("stray.txt"), "hi").unwrap();
        make_plugin(&user, "demo", &minimal("demo"));
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert_eq!(mgr.loaded().len(), 1);
    }

    #[test]
    fn roots_ordered_priority() {
        let roots = PluginRoots {
            project: Some(PathBuf::from("/p")),
            user: Some(PathBuf::from("/u")),
            managed: Some(PathBuf::from("/m")),
        };
        let ordered = roots.ordered();
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].1, PluginSource::Project);
        assert_eq!(ordered[1].1, PluginSource::User);
        assert_eq!(ordered[2].1, PluginSource::Managed);
    }

    #[test]
    fn roots_ordered_skips_none() {
        let roots = PluginRoots {
            project: None,
            user: Some(PathBuf::from("/u")),
            managed: None,
        };
        let ordered = roots.ordered();
        assert_eq!(ordered.len(), 1);
        assert_eq!(ordered[0].1, PluginSource::User);
    }

    #[test]
    fn default_for_populates_project_and_managed() {
        let ws = PathBuf::from("/workspace");
        let roots = PluginRoots::default_for(&ws);
        assert_eq!(roots.project.unwrap(), ws.join(".caliban").join("plugins"));
        assert!(roots.managed.is_some());
    }

    #[test]
    fn default_managed_dir_is_nonempty() {
        assert!(!default_managed_dir().as_os_str().is_empty());
    }

    #[test]
    fn skill_roots_returns_explicit_subdirs_when_set() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(&plug_dir).unwrap();
        fs::write(
            plug_dir.join("plugin.json"),
            r#"{ "name": "demo", "version": "0.1.0", "components": { "skills": ["skills/a", "skills/b"] } }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let sr = mgr.skill_roots();
        assert_eq!(sr.len(), 2);
        assert!(sr[0].ends_with("skills/a"));
        assert!(sr[1].ends_with("skills/b"));
    }

    #[test]
    fn agent_and_output_style_roots_default_to_subdirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        make_plugin(&user, "demo", &minimal("demo"));
        let roots = PluginRoots {
            project: None,
            user: Some(user.clone()),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert_eq!(mgr.agent_roots(), vec![user.join("demo").join("agents")]);
        assert_eq!(
            mgr.output_style_roots(),
            vec![user.join("demo").join("output-styles")]
        );
    }

    #[test]
    fn agent_and_style_roots_use_explicit_paths_when_set() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(&plug_dir).unwrap();
        fs::write(
            plug_dir.join("plugin.json"),
            r#"{ "name": "demo", "version": "0.1.0", "components": { "agents": ["agents/x.md"], "output_styles": ["styles/y.md"] } }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert!(mgr.agent_roots()[0].ends_with("agents/x.md"));
        assert!(mgr.output_style_roots()[0].ends_with("styles/y.md"));
    }

    #[test]
    fn hooks_config_skips_missing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        fs::create_dir_all(&user).unwrap();
        // Plugin with no hooks/hooks.json => no hooks config entries.
        make_plugin(&user, "demo", &minimal("demo"));
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert!(mgr.hooks_configs().is_empty());
    }

    #[test]
    fn hooks_config_skips_malformed_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(plug_dir.join("hooks")).unwrap();
        fs::write(plug_dir.join("plugin.json"), minimal("demo")).unwrap();
        fs::write(plug_dir.join("hooks").join("hooks.json"), "{ not json").unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        // Malformed hooks file is warned-and-skipped, not a hard error.
        assert!(mgr.hooks_configs().is_empty());
    }

    #[test]
    fn mcp_servers_reads_external_mcp_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(plug_dir.join("mcp")).unwrap();
        fs::write(plug_dir.join("plugin.json"), minimal("demo")).unwrap();
        fs::write(
            plug_dir.join("mcp").join(".mcp.json"),
            r#"{ "mcpServers": { "srv": { "command": "${CALIBAN_PLUGIN_ROOT}/bin/x" } } }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let servers = mgr.mcp_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "demo:srv");
        assert!(
            servers[0].1["command"]
                .as_str()
                .unwrap()
                .ends_with("/bin/x")
        );
    }

    #[test]
    fn mcp_servers_accepts_bare_object_shape() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(plug_dir.join("mcp")).unwrap();
        fs::write(plug_dir.join("plugin.json"), minimal("demo")).unwrap();
        // Bare object (no "mcpServers" wrapper) is also accepted.
        fs::write(
            plug_dir.join("mcp").join(".mcp.json"),
            r#"{ "alpha": { "command": "/bin/a" }, "beta": { "command": "/bin/b" } }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let mut names: Vec<String> = mgr.mcp_servers().into_iter().map(|(k, _)| k).collect();
        names.sort();
        assert_eq!(names, vec!["demo:alpha".to_string(), "demo:beta".into()]);
    }

    #[test]
    fn mcp_servers_skips_malformed_external_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(plug_dir.join("mcp")).unwrap();
        fs::write(plug_dir.join("plugin.json"), minimal("demo")).unwrap();
        fs::write(plug_dir.join("mcp").join(".mcp.json"), "{ broken").unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        assert!(mgr.mcp_servers().is_empty());
    }

    #[test]
    fn mcp_inline_wins_over_external_when_both_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("user");
        let plug_dir = user.join("demo");
        fs::create_dir_all(plug_dir.join("mcp")).unwrap();
        fs::write(
            plug_dir.join("plugin.json"),
            r#"{ "name": "demo", "version": "0.1.0", "mcpServers": { "inline": { "command": "/bin/i" } } }"#,
        )
        .unwrap();
        fs::write(
            plug_dir.join("mcp").join(".mcp.json"),
            r#"{ "external": { "command": "/bin/e" } }"#,
        )
        .unwrap();
        let roots = PluginRoots {
            project: None,
            user: Some(user),
            managed: None,
        };
        let mgr = PluginManager::load(&roots, &PluginSettings::default()).unwrap();
        let servers = mgr.mcp_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "demo:inline");
    }

    #[test]
    fn settings_from_env_returns_caliban_version() {
        // from_env reads only compile-time CARGO_PKG_VERSION for the version
        // field; enabled/strict come from process env which we don't mutate
        // here (hermetic). Just assert the version is populated.
        let s = PluginSettings::from_env();
        assert!(s.caliban_version.is_some());
    }
}
