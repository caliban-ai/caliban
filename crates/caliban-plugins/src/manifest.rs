//! `plugin.json` manifest schema + parser.
//!
//! See `docs/superpowers/specs/2026-05-24-plugin-system-design.md` for the
//! authoritative spec.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::PluginError;

/// Top-level `plugin.json` shape. Unknown keys are preserved in `extra`
/// to leave room for forward-compatible additions.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginManifest {
    /// Plugin name. Must match the parent directory. `^[a-z0-9_-]{1,32}$`.
    pub name: String,
    /// Semver string. Validated by [`PluginManifest::validate`].
    pub version: String,
    /// One-line description; surfaced in `/plugins` and trust prompts.
    #[serde(default)]
    pub description: String,
    /// Free-form author tag.
    #[serde(default)]
    pub author: String,
    /// SPDX license id.
    #[serde(default)]
    pub license: String,
    /// Optional homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Component-paths map (skills, hooks, agents, `output_styles`, `mcp_servers`, commands).
    #[serde(default)]
    pub components: ComponentSpec,
    /// Inline MCP server configurations. Mutually exclusive with
    /// `components.mcp_servers`; inline wins when both are present.
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers_inline: BTreeMap<String, InlineMcpServer>,
    /// Optional caliban-specific gating.
    #[serde(default)]
    pub caliban: CalibanRequirements,
    /// Unknown manifest keys (forward-compat).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Component paths relative to the plugin root. Each value is *either* a
/// string (one path) or an array (multiple). Missing values default to
/// "discover everything in the conventional subdirectory" — see the spec
/// for which fields auto-discover.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ComponentSpec {
    /// Skill subdirectories. Each entry should be a directory containing
    /// `SKILL.md`. When unset, the loader scans `skills/*/SKILL.md`.
    pub skills: Option<PathList>,
    /// Hook config file (defaults to `hooks/hooks.json`).
    pub hooks: Option<PathList>,
    /// Sub-agent `.md` files (defaults to `agents/*.md`).
    pub agents: Option<PathList>,
    /// Output style `.md` files (defaults to `output-styles/*.md`).
    pub output_styles: Option<PathList>,
    /// MCP server config file (defaults to `mcp/.mcp.json`).
    pub mcp_servers: Option<PathList>,
    /// Optional slash-command markdown files (deferred to ADR 0040).
    pub commands: Option<PathList>,
}

/// A "string-or-list-of-strings" wrapper used by every `components.*` field.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PathList {
    /// Single path.
    Single(String),
    /// Multiple paths.
    Many(Vec<String>),
}

impl PathList {
    /// Return paths as a Vec.
    #[must_use]
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s.clone()],
            Self::Many(v) => v.clone(),
        }
    }
}

/// Inline MCP server config block under the top-level `mcpServers` key.
/// Mirrors the JSON shape Claude Code uses; not a full toml-mcp parse.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InlineMcpServer {
    /// Executable path (after `${CALIBAN_PLUGIN_ROOT}` expansion).
    pub command: String,
    /// CLI args.
    #[serde(default)]
    pub args: Vec<String>,
    /// Env-var overrides.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Optional transport hint (`stdio` is default).
    #[serde(default)]
    pub transport: Option<String>,
}

/// Caliban-specific requirements / filters.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CalibanRequirements {
    /// Skip the plugin when the running caliban is older than this semver.
    pub min_version: Option<String>,
    /// Limit plugin to these platforms (`macos`, `linux`, `windows`).
    pub platforms: Option<Vec<String>>,
}

impl PluginManifest {
    /// Parse a manifest from raw JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Parse`] on JSON syntax errors and
    /// [`PluginError::Invalid`] on validation failures.
    pub fn from_json(raw: &str, path: &Path) -> Result<Self, PluginError> {
        let mf: Self = serde_json::from_str(raw).map_err(|source| PluginError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        mf.validate(path)?;
        Ok(mf)
    }

    /// Read and parse a manifest from disk.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Io`] on read failures, plus the errors
    /// surfaced by [`PluginManifest::from_json`].
    pub fn from_path(path: &Path) -> Result<Self, PluginError> {
        let raw = std::fs::read_to_string(path).map_err(|source| PluginError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&raw, path)
    }

    /// Validate the manifest in isolation (no on-disk component checks).
    ///
    /// Performs name-regex, semver, and `caliban.min_version`/`platforms`
    /// shape checks. Does *not* check that `name` matches the parent dir
    /// — that's [`PluginManifest::check_name_matches_dir`].
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Invalid`] when any check fails.
    pub fn validate(&self, path: &Path) -> Result<(), PluginError> {
        if !is_valid_name(&self.name) {
            return Err(PluginError::Invalid {
                path: path.to_path_buf(),
                message: format!(
                    "invalid name '{}': must match [a-z0-9_-]{{1,32}} and be lowercase",
                    self.name
                ),
            });
        }
        // Version itself must parse as semver.
        semver::Version::parse(&self.version).map_err(|e| PluginError::Invalid {
            path: path.to_path_buf(),
            message: format!("invalid version '{}': {e}", self.version),
        })?;
        if let Some(min) = self.caliban.min_version.as_deref() {
            // Accept partial versions like "0.5" by widening to semver-req.
            semver::VersionReq::parse(&format!(">={min}")).map_err(|e| PluginError::Invalid {
                path: path.to_path_buf(),
                message: format!("invalid caliban.min_version '{min}': {e}"),
            })?;
        }
        if let Some(ps) = self.caliban.platforms.as_ref() {
            for p in ps {
                if !matches!(p.as_str(), "macos" | "linux" | "windows") {
                    return Err(PluginError::Invalid {
                        path: path.to_path_buf(),
                        message: format!(
                            "invalid caliban.platforms entry '{p}': must be macos|linux|windows"
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Confirm the on-disk parent directory's name matches `self.name`.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NameMismatch`] when the parent dir name and
    /// manifest name disagree.
    pub fn check_name_matches_dir(&self, manifest_path: &Path) -> Result<(), PluginError> {
        let dir_name = manifest_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if dir_name == self.name {
            Ok(())
        } else {
            Err(PluginError::NameMismatch {
                manifest_name: self.name.clone(),
                dir_name,
                path: manifest_path.to_path_buf(),
            })
        }
    }

    /// Return true if the manifest applies to the running platform.
    #[must_use]
    pub fn platform_matches(&self) -> bool {
        let Some(allowed) = self.caliban.platforms.as_ref() else {
            return true;
        };
        allowed.iter().any(|p| p == current_platform())
    }

    /// Resolve `components.*` entries to absolute paths under `root`.
    /// Missing files are *not* an error here — the downstream loader
    /// decides whether to warn or skip.
    #[must_use]
    pub fn resolved_components(&self, root: &Path) -> ResolvedComponents {
        let resolve_list = |pl: &Option<PathList>| -> Vec<PathBuf> {
            pl.as_ref()
                .map(PathList::as_vec)
                .unwrap_or_default()
                .into_iter()
                .map(|s| root.join(s))
                .collect()
        };
        ResolvedComponents {
            skills: resolve_list(&self.components.skills),
            hooks: resolve_list(&self.components.hooks),
            agents: resolve_list(&self.components.agents),
            output_styles: resolve_list(&self.components.output_styles),
            mcp_servers: resolve_list(&self.components.mcp_servers),
            commands: resolve_list(&self.components.commands),
        }
    }
}

/// Resolved component paths (all absolute under the plugin root).
#[derive(Debug, Clone, Default)]
pub struct ResolvedComponents {
    /// Skill directories (each contains `SKILL.md`).
    pub skills: Vec<PathBuf>,
    /// Hook config files.
    pub hooks: Vec<PathBuf>,
    /// Sub-agent `.md` files.
    pub agents: Vec<PathBuf>,
    /// Output-style `.md` files.
    pub output_styles: Vec<PathBuf>,
    /// MCP server config files.
    pub mcp_servers: Vec<PathBuf>,
    /// Slash command files.
    pub commands: Vec<PathBuf>,
}

/// Plugin name regex check (`^[a-z0-9_-]{1,32}$`).
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Return the static platform string used by `caliban.platforms`.
#[must_use]
pub fn current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_minimal_manifest() {
        let raw = r#"{ "name": "demo", "version": "0.1.0", "description": "demo plugin" }"#;
        let mf = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap();
        assert_eq!(mf.name, "demo");
        assert_eq!(mf.version, "0.1.0");
        assert_eq!(mf.description, "demo plugin");
        assert!(mf.components.skills.is_none());
    }

    #[test]
    fn parses_full_manifest() {
        let raw = r#"{
            "name": "superpowers",
            "version": "1.4.2",
            "description": "Curated skills",
            "author": "alice <alice@example.com>",
            "license": "MIT",
            "homepage": "https://example.com",
            "components": {
                "skills": ["skills/foo", "skills/bar"],
                "hooks": "hooks/hooks.json",
                "agents": ["agents/reviewer.md"],
                "output_styles": "output-styles/learning.md",
                "mcp_servers": "mcp/.mcp.json",
                "commands": ["commands/recap.md"]
            },
            "mcpServers": {
                "fixtures": {
                    "command": "${CALIBAN_PLUGIN_ROOT}/bin/server",
                    "args": ["--verbose"]
                }
            },
            "caliban": { "min_version": "0.5.0", "platforms": ["macos", "linux"] }
        }"#;
        let mf = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap();
        assert_eq!(mf.author, "alice <alice@example.com>");
        let skills = mf.components.skills.as_ref().unwrap().as_vec();
        assert_eq!(skills, vec!["skills/foo".to_string(), "skills/bar".into()]);
        let agents = mf.components.agents.as_ref().unwrap().as_vec();
        assert_eq!(agents, vec!["agents/reviewer.md".to_string()]);
        let hooks = mf.components.hooks.as_ref().unwrap().as_vec();
        assert_eq!(hooks, vec!["hooks/hooks.json".to_string()]);
        assert_eq!(mf.mcp_servers_inline.len(), 1);
        assert_eq!(mf.caliban.platforms.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn invalid_json_is_parse_error() {
        let raw = r"not json at all";
        let err = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap_err();
        assert!(matches!(err, PluginError::Parse { .. }));
    }

    #[test]
    fn name_regex_enforced() {
        assert!(is_valid_name("demo"));
        assert!(is_valid_name("demo-1_x"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("UPPER"));
        assert!(!is_valid_name("with space"));
        assert!(!is_valid_name(&"x".repeat(33)));
        assert!(!is_valid_name("dot.name"));
    }

    #[test]
    fn invalid_name_rejected_in_manifest() {
        let raw = r#"{ "name": "BAD", "version": "0.1.0" }"#;
        let err = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap_err();
        assert!(matches!(err, PluginError::Invalid { .. }));
    }

    #[test]
    fn invalid_semver_rejected() {
        let raw = r#"{ "name": "demo", "version": "not.a.version" }"#;
        let err = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap_err();
        assert!(matches!(err, PluginError::Invalid { .. }));
    }

    #[test]
    fn unknown_platform_rejected() {
        let raw = r#"{
            "name": "demo", "version": "0.1.0",
            "caliban": { "platforms": ["beos"] }
        }"#;
        let err = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap_err();
        assert!(matches!(err, PluginError::Invalid { .. }));
    }

    #[test]
    fn check_name_matches_dir_ok() {
        let raw = r#"{ "name": "demo", "version": "0.1.0" }"#;
        let mf = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let plug_dir = tmp.path().join("demo");
        std::fs::create_dir_all(&plug_dir).unwrap();
        let manifest_path = plug_dir.join("plugin.json");
        std::fs::write(&manifest_path, raw).unwrap();
        mf.check_name_matches_dir(&manifest_path).unwrap();
    }

    #[test]
    fn check_name_mismatch_errors() {
        let raw = r#"{ "name": "demo", "version": "0.1.0" }"#;
        let mf = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let plug_dir = tmp.path().join("wrong");
        std::fs::create_dir_all(&plug_dir).unwrap();
        let manifest_path = plug_dir.join("plugin.json");
        std::fs::write(&manifest_path, raw).unwrap();
        let err = mf.check_name_matches_dir(&manifest_path).unwrap_err();
        assert!(matches!(err, PluginError::NameMismatch { .. }));
    }

    #[test]
    fn unknown_fields_preserved() {
        let raw = r#"{
            "name": "demo", "version": "0.1.0",
            "future_field": { "anything": [1, 2, 3] }
        }"#;
        let mf = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap();
        assert!(mf.extra.contains_key("future_field"));
    }

    #[test]
    fn resolves_components_to_absolute_paths() {
        let raw = r#"{
            "name": "demo", "version": "0.1.0",
            "components": { "skills": ["skills/a", "skills/b"] }
        }"#;
        let mf = PluginManifest::from_json(raw, Path::new("plugin.json")).unwrap();
        let root = Path::new("/plugins/demo");
        let rc = mf.resolved_components(root);
        assert_eq!(rc.skills.len(), 2);
        assert_eq!(rc.skills[0], root.join("skills/a"));
        assert_eq!(rc.skills[1], root.join("skills/b"));
    }

    #[test]
    fn platform_matches_filters() {
        let raw_other = r#"{
            "name": "demo", "version": "0.1.0",
            "caliban": { "platforms": ["windows"] }
        }"#;
        let mf = PluginManifest::from_json(raw_other, Path::new("plugin.json")).unwrap();
        #[cfg(not(target_os = "windows"))]
        assert!(!mf.platform_matches());
        let raw_unset = r#"{ "name": "demo", "version": "0.1.0" }"#;
        let mf2 = PluginManifest::from_json(raw_unset, Path::new("plugin.json")).unwrap();
        assert!(mf2.platform_matches());
    }
}
