//! `caliban plugin {install,list,enable,disable,remove,info,update}` impl.
//!
//! The CLI sub-binary delegates each subcommand into a free function that
//! takes a configurable `Cli` context (so tests can drive it without spawning
//! a subprocess). The binary wires these into `clap` in `caliban/src/main.rs`.

use std::path::{Path, PathBuf};

use crate::error::PluginError;
use crate::manager::{PluginManager, PluginRoots, PluginSettings};
use crate::marketplace::{MarketplaceClient, TrustDecision};
use crate::trust::TrustStore;

/// CLI execution context. Tests construct one directly; the binary
/// constructs one from `clap` args.
#[derive(Debug, Clone)]
pub struct Cli {
    /// Workspace root (used for project-scope discovery).
    pub workspace_root: PathBuf,
    /// User-scope install dir (where `install` and `remove` operate).
    pub user_install_dir: PathBuf,
    /// Trust store paths.
    pub trust: TrustStore,
    /// Marketplace client.
    pub marketplace: MarketplaceClient,
    /// Manager settings.
    pub settings: PluginSettings,
}

/// Outcome row shown by `list`.
#[derive(Debug, Clone)]
pub struct ListedPlugin {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Source root.
    pub source: String,
    /// Enabled flag (from settings).
    pub enabled: bool,
    /// Counts string ("3 skills · 1 hook").
    pub summary: String,
}

impl Cli {
    /// `caliban plugin list` — return one row per installed plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] only on unrecoverable IO (e.g. unreadable
    /// plugin root). Per-plugin parse errors are listed but don't fail
    /// the call.
    pub fn list(&self) -> Result<Vec<ListedPlugin>, PluginError> {
        let roots = PluginRoots {
            project: Some(self.workspace_root.join(".caliban").join("plugins")),
            user: Some(self.user_install_dir.clone()),
            managed: Some(crate::manager::default_managed_dir()),
        };
        // Use an unfiltered settings clone (clear enabled list) so list shows
        // everything installed.
        let mut s = self.settings.clone();
        s.enabled = None;
        let mgr = PluginManager::load(&roots, &s)?;
        let mut out = Vec::new();
        for p in mgr.loaded() {
            let enabled = self
                .settings
                .enabled
                .as_ref()
                .is_none_or(|list| list.iter().any(|n| n == &p.manifest.name));
            out.push(ListedPlugin {
                name: p.manifest.name.clone(),
                version: p.manifest.version.clone(),
                source: p.source.as_str().to_string(),
                enabled,
                summary: summarize_components(p),
            });
        }
        for f in mgr.failures() {
            out.push(ListedPlugin {
                name: f.dir_name.clone(),
                version: "?".into(),
                source: f.source.as_str().to_string(),
                enabled: false,
                summary: format!("invalid: {}", f.error),
            });
        }
        Ok(out)
    }

    /// `caliban plugin info <name>` — return the manifest as JSON.
    ///
    /// # Errors
    ///
    /// [`PluginError::PluginNotFound`] when no plugin with that name is
    /// installed.
    pub fn info(&self, name: &str) -> Result<serde_json::Value, PluginError> {
        let roots = PluginRoots {
            project: Some(self.workspace_root.join(".caliban").join("plugins")),
            user: Some(self.user_install_dir.clone()),
            managed: Some(crate::manager::default_managed_dir()),
        };
        let mut s = self.settings.clone();
        s.enabled = None;
        let mgr = PluginManager::load(&roots, &s)?;
        let p = mgr
            .loaded()
            .iter()
            .find(|p| p.manifest.name == name)
            .ok_or_else(|| PluginError::PluginNotFound {
                name: name.to_string(),
                url: "(installed)".into(),
            })?;
        let v = serde_json::to_value(&p.manifest).map_err(|source| PluginError::Parse {
            path: p.root_dir.join("plugin.json"),
            source,
        })?;
        Ok(v)
    }

    /// `caliban plugin remove <name>` — delete the user-scope install
    /// directory and clear the trust record.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Io`] on filesystem failure, or
    /// [`PluginError::PluginNotFound`] if the plugin isn't installed.
    pub fn remove(&mut self, name: &str) -> Result<(), PluginError> {
        let dir = self.user_install_dir.join(name);
        if !dir.exists() {
            return Err(PluginError::PluginNotFound {
                name: name.to_string(),
                url: "(installed)".into(),
            });
        }
        std::fs::remove_dir_all(&dir).map_err(|source| PluginError::Io {
            path: dir.clone(),
            source,
        })?;
        self.trust.forget(name);
        self.trust.save()?;
        Ok(())
    }

    /// `caliban plugin install <name>@<marketplace>` — full install flow.
    ///
    /// # Errors
    ///
    /// See [`MarketplaceClient::install`].
    pub async fn install(
        &mut self,
        name: &str,
        marketplace_url: &str,
        desired_version: Option<&str>,
        approve: bool,
    ) -> Result<PathBuf, PluginError> {
        let decision = if approve {
            TrustDecision::Approve
        } else {
            TrustDecision::UseCache
        };
        std::fs::create_dir_all(&self.user_install_dir).map_err(|source| PluginError::Io {
            path: self.user_install_dir.clone(),
            source,
        })?;
        self.marketplace
            .install(
                name,
                marketplace_url,
                desired_version,
                &self.user_install_dir,
                &mut self.trust,
                decision,
            )
            .await
    }

    /// `caliban plugin update <name>` — re-fetch the marketplace index and
    /// reinstall if the remote version is newer than the local trust
    /// record.
    ///
    /// # Errors
    ///
    /// See [`MarketplaceClient::install`].
    pub async fn update(
        &mut self,
        name: &str,
        approve: bool,
    ) -> Result<Option<PathBuf>, PluginError> {
        let rec = self
            .trust
            .get(name)
            .cloned()
            .ok_or_else(|| PluginError::PluginNotFound {
                name: name.to_string(),
                url: "(installed)".into(),
            })?;
        let index = self.marketplace.fetch_index(&rec.marketplace).await?;
        let entry = index
            .plugins
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| PluginError::PluginNotFound {
                name: name.to_string(),
                url: rec.marketplace.clone(),
            })?;
        let latest = entry.latest_version().ok_or_else(|| PluginError::Invalid {
            path: PathBuf::from(&rec.marketplace),
            message: format!("no version metadata for plugin '{name}'"),
        })?;
        if version_lte(&latest.version, &rec.version) {
            tracing::info!(
                target: caliban_common::tracing_targets::TARGET_PLUGINS,
                name = name,
                local = %rec.version,
                remote = %latest.version,
                "plugin update: local is up-to-date",
            );
            return Ok(None);
        }
        let path = self
            .install(name, &rec.marketplace, Some(&latest.version), approve)
            .await?;
        Ok(Some(path))
    }
}

fn version_lte(latest: &str, local: &str) -> bool {
    match (
        semver::Version::parse(latest),
        semver::Version::parse(local),
    ) {
        (Ok(a), Ok(b)) => a <= b,
        _ => latest == local,
    }
}

fn summarize_components(p: &crate::loaded::LoadedPlugin) -> String {
    let mut parts: Vec<String> = Vec::new();
    let skills = if p.components.skills.is_empty() {
        count_dir(&p.root_dir.join("skills"))
    } else {
        p.components.skills.len()
    };
    if skills > 0 {
        parts.push(format!("{skills} skill{}", plural(skills)));
    }
    let hooks = if p.components.hooks.is_empty() {
        usize::from(p.root_dir.join("hooks").join("hooks.json").exists())
    } else {
        p.components.hooks.len()
    };
    if hooks > 0 {
        parts.push(format!("{hooks} hook{}", plural(hooks)));
    }
    let agents = if p.components.agents.is_empty() {
        count_dir(&p.root_dir.join("agents"))
    } else {
        p.components.agents.len()
    };
    if agents > 0 {
        parts.push(format!("{agents} agent{}", plural(agents)));
    }
    let styles = if p.components.output_styles.is_empty() {
        count_dir(&p.root_dir.join("output-styles"))
    } else {
        p.components.output_styles.len()
    };
    if styles > 0 {
        parts.push(format!("{styles} style{}", plural(styles)));
    }
    let mcps = if p.components.mcp_servers.is_empty() {
        usize::from(p.root_dir.join("mcp").join(".mcp.json").exists())
            + p.manifest.mcp_servers_inline.len()
    } else {
        p.components.mcp_servers.len()
    };
    if mcps > 0 {
        parts.push(format!("{mcps} mcp"));
    }
    parts.join(" \u{00b7} ")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn count_dir(p: &Path) -> usize {
    p.read_dir().map_or(0, |rd| rd.flatten().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_plugin(root: &Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            format!(r#"{{ "name": "{name}", "version": "0.1.0", "description": "x" }}"#),
        )
        .unwrap();
    }

    fn make_cli(tmp: &Path) -> Cli {
        let user_dir = tmp.join("user");
        let ws = tmp.join("ws");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        Cli {
            workspace_root: ws,
            user_install_dir: user_dir,
            trust: TrustStore::open(tmp.join("trust.json"), tmp.join("allow.json")).unwrap(),
            marketplace: MarketplaceClient::default(),
            settings: PluginSettings::default(),
        }
    }

    #[test]
    fn list_returns_installed_plugins() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        make_plugin(&cli.user_install_dir, "demo");
        let rows = cli.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "demo");
        assert!(rows[0].enabled);
    }

    #[test]
    fn info_returns_manifest_value() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        make_plugin(&cli.user_install_dir, "demo");
        let v = cli.info("demo").unwrap();
        assert_eq!(v["name"], "demo");
        assert_eq!(v["version"], "0.1.0");
    }

    #[test]
    fn info_missing_plugin_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        let err = cli.info("does-not-exist").unwrap_err();
        assert!(matches!(err, PluginError::PluginNotFound { .. }));
    }

    #[test]
    fn remove_deletes_install_dir_and_clears_trust() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cli = make_cli(tmp.path());
        make_plugin(&cli.user_install_dir, "demo");
        cli.trust.record(
            "demo",
            crate::trust::PluginTrustRecord {
                version: "0.1.0".into(),
                marketplace: "https://m/idx".into(),
                manifest_sha256: "abc".into(),
                installed_at: "now".into(),
            },
        );
        cli.remove("demo").unwrap();
        assert!(!cli.user_install_dir.join("demo").exists());
        assert!(cli.trust.get("demo").is_none());
    }

    #[test]
    fn list_includes_disabled_status() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cli = make_cli(tmp.path());
        make_plugin(&cli.user_install_dir, "demo");
        make_plugin(&cli.user_install_dir, "off");
        cli.settings.enabled = Some(vec!["demo".to_string()]);
        let rows = cli.list().unwrap();
        let demo = rows.iter().find(|r| r.name == "demo").unwrap();
        let off = rows.iter().find(|r| r.name == "off").unwrap();
        assert!(demo.enabled);
        assert!(!off.enabled);
    }

    #[test]
    fn list_empty_when_nothing_installed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        let rows = cli.list().unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_reports_invalid_manifest_as_failure_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        // A plugin dir with malformed JSON becomes a failure row, not a load.
        let dir = cli.user_install_dir.join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), "{ not json").unwrap();
        let rows = cli.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "broken");
        assert_eq!(rows[0].version, "?");
        assert!(!rows[0].enabled);
        assert!(rows[0].summary.starts_with("invalid:"));
    }

    #[test]
    fn list_discovers_project_scope_plugin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        let project_root = cli.workspace_root.join(".caliban").join("plugins");
        std::fs::create_dir_all(&project_root).unwrap();
        make_plugin(&project_root, "proj");
        let rows = cli.list().unwrap();
        let row = rows.iter().find(|r| r.name == "proj").unwrap();
        assert_eq!(row.source, "project");
    }

    #[test]
    fn list_summary_counts_skill_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        make_plugin(&cli.user_install_dir, "demo");
        // Auto-discovered skills/ subdirectory contributes to the summary.
        let skills = cli.user_install_dir.join("demo").join("skills");
        std::fs::create_dir_all(skills.join("alpha")).unwrap();
        std::fs::create_dir_all(skills.join("beta")).unwrap();
        let rows = cli.list().unwrap();
        let row = rows.iter().find(|r| r.name == "demo").unwrap();
        assert!(row.summary.contains("2 skills"), "summary={}", row.summary);
    }

    #[test]
    fn info_serializes_full_manifest_fields() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cli = make_cli(tmp.path());
        let dir = cli.user_install_dir.join("rich");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{ "name": "rich", "version": "2.3.4", "description": "d", "author": "a", "license": "MIT" }"#,
        )
        .unwrap();
        let v = cli.info("rich").unwrap();
        assert_eq!(v["version"], "2.3.4");
        assert_eq!(v["author"], "a");
        assert_eq!(v["license"], "MIT");
    }

    #[test]
    fn remove_missing_plugin_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cli = make_cli(tmp.path());
        let err = cli.remove("ghost").unwrap_err();
        assert!(matches!(err, PluginError::PluginNotFound { .. }));
    }

    #[test]
    fn remove_without_trust_record_still_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cli = make_cli(tmp.path());
        make_plugin(&cli.user_install_dir, "demo");
        // No trust record recorded; forget() on a missing key is a no-op.
        cli.remove("demo").unwrap();
        assert!(!cli.user_install_dir.join("demo").exists());
    }

    #[tokio::test]
    async fn update_unknown_plugin_errors_without_network() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cli = make_cli(tmp.path());
        // No trust record for "ghost" => fails before any network call.
        let err = cli.update("ghost", false).await.unwrap_err();
        assert!(matches!(err, PluginError::PluginNotFound { .. }));
    }

    #[test]
    fn version_lte_semver_ordering() {
        assert!(version_lte("1.0.0", "1.0.0"));
        assert!(version_lte("1.0.0", "1.0.1"));
        assert!(!version_lte("1.0.1", "1.0.0"));
        // Non-semver falls back to string equality.
        assert!(version_lte("nope", "nope"));
        assert!(!version_lte("nope", "other"));
    }

    #[test]
    fn plural_suffix() {
        assert_eq!(plural(1), "");
        assert_eq!(plural(0), "s");
        assert_eq!(plural(2), "s");
    }

    #[test]
    fn count_dir_counts_entries_and_handles_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "x").unwrap();
        std::fs::write(dir.join("b"), "y").unwrap();
        assert_eq!(count_dir(&dir), 2);
        assert_eq!(count_dir(&tmp.path().join("missing")), 0);
    }
}
