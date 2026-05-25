//! Plugin discovery + filter + namespacing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::PluginError;
use crate::expand;
use crate::loaded::{LoadedPlugin, PluginSource};
use crate::manifest::PluginManifest;

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
        let user = dirs::data_local_dir().map(|d| d.join("caliban").join("plugins"));
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

        for (root, source) in roots.ordered() {
            if !root.exists() {
                continue;
            }
            let rd = match std::fs::read_dir(&root) {
                Ok(rd) => rd,
                Err(source_err) => {
                    return Err(PluginError::Io {
                        path: root.clone(),
                        source: source_err,
                    });
                }
            };
            for entry in rd.flatten() {
                let plug_dir = entry.path();
                if !plug_dir.is_dir() {
                    continue;
                }
                let dir_name = plug_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let manifest_path = plug_dir.join("plugin.json");
                if !manifest_path.exists() {
                    continue;
                }
                match Self::try_load_one(&plug_dir, &manifest_path, source, settings) {
                    Ok(Some(p)) => {
                        if let Some(existing) = by_name.get(&p.manifest.name) {
                            tracing::debug!(
                                target: "caliban::plugins",
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
                            root_dir: plug_dir.clone(),
                            source,
                            dir_name,
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

    fn try_load_one(
        plug_dir: &Path,
        manifest_path: &Path,
        source: PluginSource,
        settings: &PluginSettings,
    ) -> Result<Option<LoadedPlugin>, PluginError> {
        let manifest = PluginManifest::from_path(manifest_path)?;
        manifest.check_name_matches_dir(manifest_path)?;
        // Platform gating.
        if !manifest.platform_matches() {
            tracing::info!(
                target: "caliban::plugins",
                name = %manifest.name,
                "skipping plugin: platform mismatch",
            );
            return Ok(None);
        }
        // Min-version gating.
        if let (Some(min), Some(cur)) = (
            manifest.caliban.min_version.as_deref(),
            settings.caliban_version.as_deref(),
        ) && let (Ok(min_v), Ok(cur_v)) = (
            semver::Version::parse(&pad_version(min)),
            semver::Version::parse(&pad_version(cur)),
        ) && cur_v < min_v
        {
            tracing::info!(
                target: "caliban::plugins",
                name = %manifest.name,
                min = %min,
                current = %cur,
                "skipping plugin: caliban version too old",
            );
            return Ok(None);
        }

        // Strict-plugin-only-customization: reject non-managed plugins.
        if settings.strict_plugin_only_customization && source != PluginSource::Managed {
            return Err(PluginError::StrictPluginOnly {
                name: manifest.name.clone(),
            });
        }

        // Enable list filter (managed plugins ignore it).
        if source != PluginSource::Managed
            && let Some(enabled) = settings.enabled.as_ref()
            && !enabled.iter().any(|n| n == &manifest.name)
        {
            tracing::debug!(
                target: "caliban::plugins",
                name = %manifest.name,
                "skipping plugin: not in CALIBAN_ENABLED_PLUGINS",
            );
            return Ok(None);
        }

        let components = manifest.resolved_components(plug_dir);
        Ok(Some(LoadedPlugin {
            namespace: manifest.name.clone(),
            manifest,
            root_dir: plug_dir.to_path_buf(),
            source,
            components,
        }))
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
        let mut out = Vec::new();
        for p in &self.plugins {
            if p.components.skills.is_empty() {
                out.push(p.root_dir.join("skills"));
            } else {
                out.extend(p.components.skills.iter().cloned());
            }
        }
        out
    }

    /// Same as [`skill_roots`] for output styles. Returned paths are
    /// *directories* containing `.md` files; if the manifest enumerated
    /// individual files, those file paths are returned as-is.
    #[must_use]
    pub fn output_style_roots(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for p in &self.plugins {
            if p.components.output_styles.is_empty() {
                out.push(p.root_dir.join("output-styles"));
            } else {
                out.extend(p.components.output_styles.iter().cloned());
            }
        }
        out
    }

    /// Same as [`skill_roots`] for agents.
    #[must_use]
    pub fn agent_roots(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for p in &self.plugins {
            if p.components.agents.is_empty() {
                out.push(p.root_dir.join("agents"));
            } else {
                out.extend(p.components.agents.iter().cloned());
            }
        }
        out
    }

    /// Merged hooks config across all loaded plugins. Each plugin's
    /// hooks file is read, `${CALIBAN_PLUGIN_ROOT}` expanded, and the
    /// resulting `serde_json::Value` returned in load order. The downstream
    /// hooks loader is responsible for merging into its TOML world.
    #[must_use]
    pub fn hooks_configs(&self) -> Vec<(String, serde_json::Value)> {
        let mut out = Vec::new();
        for p in &self.plugins {
            let candidates: Vec<PathBuf> = if p.components.hooks.is_empty() {
                vec![p.root_dir.join("hooks").join("hooks.json")]
            } else {
                p.components.hooks.clone()
            };
            for path in candidates {
                if !path.exists() {
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                        Ok(mut v) => {
                            expand::expand_json_in_place(&mut v, &p.root_dir);
                            out.push((p.namespace.clone(), v));
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "caliban::plugins",
                                path = %path.display(),
                                error = %e,
                                "skipping malformed plugin hooks.json",
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            target: "caliban::plugins",
                            path = %path.display(),
                            error = %e,
                            "could not read plugin hooks.json",
                        );
                    }
                }
            }
        }
        out
    }

    /// Merged MCP server configs across plugins. Inline `mcpServers` block
    /// wins over `components.mcp_servers` when both are present (with a
    /// warning). Each server name is namespaced `<plugin>:<server>`.
    #[must_use]
    pub fn mcp_servers(&self) -> Vec<(String, serde_json::Value)> {
        let mut out = Vec::new();
        for p in &self.plugins {
            let has_inline = !p.manifest.mcp_servers_inline.is_empty();
            let has_external = !p.components.mcp_servers.is_empty()
                || p.root_dir.join("mcp").join(".mcp.json").exists();
            if has_inline && has_external {
                tracing::warn!(
                    target: "caliban::plugins",
                    plugin = %p.namespace,
                    "both inline mcpServers and components.mcp_servers set; inline wins",
                );
            }
            if has_inline {
                for (srv_name, srv) in &p.manifest.mcp_servers_inline {
                    let key = format!("{}:{srv_name}", p.namespace);
                    let mut v = serde_json::to_value(srv).unwrap_or(serde_json::Value::Null);
                    expand::expand_json_in_place(&mut v, &p.root_dir);
                    out.push((key, v));
                }
            } else {
                let candidates: Vec<PathBuf> = if p.components.mcp_servers.is_empty() {
                    let candidate = p.root_dir.join("mcp").join(".mcp.json");
                    if candidate.exists() {
                        vec![candidate]
                    } else {
                        Vec::new()
                    }
                } else {
                    p.components.mcp_servers.clone()
                };
                for path in candidates {
                    if !path.exists() {
                        continue;
                    }
                    match std::fs::read_to_string(&path) {
                        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                            Ok(v) => {
                                Self::flatten_mcp_json(&mut out, &p.namespace, &v, &p.root_dir);
                            }
                            Err(e) => tracing::warn!(
                                target: "caliban::plugins",
                                path = %path.display(),
                                error = %e,
                                "skipping malformed plugin .mcp.json",
                            ),
                        },
                        Err(e) => tracing::warn!(
                            target: "caliban::plugins",
                            path = %path.display(),
                            error = %e,
                            "could not read plugin .mcp.json",
                        ),
                    }
                }
            }
        }
        out
    }

    /// Flatten `{"mcpServers": {"a": {...}, "b": {...}}}` (Claude Code shape)
    /// or `{"a": {...}}` (bare) into namespaced entries.
    fn flatten_mcp_json(
        out: &mut Vec<(String, serde_json::Value)>,
        namespace: &str,
        v: &serde_json::Value,
        root: &Path,
    ) {
        // Accept either `{"mcpServers": {...}}` or a bare object of servers.
        let map = if let Some(inner) = v.get("mcpServers").and_then(|x| x.as_object()) {
            inner.clone()
        } else if let Some(obj) = v.as_object() {
            obj.clone()
        } else {
            return;
        };
        for (srv_name, mut srv) in map {
            expand::expand_json_in_place(&mut srv, root);
            out.push((format!("{namespace}:{srv_name}"), srv));
        }
    }
}

/// Pad a "0.5" → "0.5.0" so semver parses it.
fn pad_version(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => v.to_string(),
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
}
