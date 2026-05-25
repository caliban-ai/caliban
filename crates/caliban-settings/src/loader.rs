//! Layered loader.
//!
//! Drives the four canonical scopes + the CLI overlay, merges them per
//! [`crate::merge::merge_values`], schema-validates the result, and
//! deserializes into [`Settings`]. Also handles the
//! `parent_settings_behavior: "block"` flip and the JSON/TOML coexistence
//! rule.

use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::merge::merge_values;
use crate::schema::validate_value;
use crate::scope::{Scope, ScopePaths};
use crate::settings::Settings;

/// Where the loader found settings for one scope.
#[derive(Debug, Clone)]
pub struct ScopeSource {
    /// Which scope.
    pub scope: Scope,
    /// Path the data was loaded from. `None` for the CLI overlay.
    pub path: Option<PathBuf>,
    /// File format used. `None` for an empty / missing scope.
    pub format: Option<&'static str>,
}

/// Options driving [`load_settings`].
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Workspace root for project/local scope discovery.
    pub workspace_root: PathBuf,
    /// Override OS-default discovery paths (managed dir, user-config
    /// dir). Tests use this to inject fakes.
    pub paths: ScopePaths,
    /// When set, restricts which scopes are loaded.
    pub scope_filter: Option<Vec<Scope>>,
    /// CLI overlay: parsed-and-injected above local (below CLI flag
    /// rules like `--allow`).
    pub cli_overlay: Option<Value>,
    /// `--bare` mode: skip *all* settings discovery; return an empty
    /// `Settings`.
    pub bare: bool,
    /// Run schema validation; default `true`. Invalid documents warn
    /// but don't fail.
    pub schema_validate: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            workspace_root: PathBuf::new(),
            paths: ScopePaths::defaults(),
            scope_filter: None,
            cli_overlay: None,
            bare: false,
            schema_validate: true,
        }
    }
}

impl LoadOptions {
    /// Convenient constructor.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            paths: ScopePaths::defaults(),
            scope_filter: None,
            cli_overlay: None,
            bare: false,
            schema_validate: true,
        }
    }

    /// Apply `--setting-sources <CSV>`.
    #[must_use]
    pub fn with_sources_csv(mut self, csv: &str) -> Self {
        let mut filter = Vec::new();
        for part in csv.split(',') {
            if let Some(scope) = Scope::parse(part) {
                filter.push(scope);
            } else {
                tracing::warn!(target: caliban_common::tracing_targets::TARGET_SETTINGS, entry = part, "unknown --setting-sources entry; ignoring");
            }
        }
        self.scope_filter = Some(filter);
        self
    }

    /// Apply `--settings <FILE|JSON>`. The argument may be a path to a
    /// `.json` / `.toml` file or an inline JSON object.
    ///
    /// # Errors
    /// Propagates parse errors from JSON or TOML decoders. File-IO
    /// errors are returned verbatim.
    pub fn with_cli_overlay(mut self, arg: &str) -> Result<Self, LoadError> {
        let value = parse_cli_overlay(arg)?;
        self.cli_overlay = Some(value);
        Ok(self)
    }
}

/// Outcome of the load operation.
#[derive(Debug)]
pub struct LoadOutcome {
    /// The fully-merged, typed settings.
    pub settings: Settings,
    /// Per-scope provenance for the `/config` overlay.
    pub sources: Vec<ScopeSource>,
    /// Schema-validation warnings collected per scope (empty when
    /// valid).
    pub validation_warnings: Vec<String>,
}

/// Errors emitted by the loader.
#[derive(Error, Debug)]
pub enum LoadError {
    /// IO failure reading a file.
    #[error("settings: io error reading {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// JSON parse error.
    #[error("settings: json parse error in {path}: {source}")]
    ParseJson {
        /// Path that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// TOML parse error.
    #[error("settings: toml parse error in {path}: {source}")]
    ParseToml {
        /// Path that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
    /// Final-deserialization error (merged value didn't fit `Settings`).
    #[error("settings: deserialize error: {0}")]
    Final(#[source] serde_json::Error),
    /// CLI overlay couldn't be interpreted as JSON or as a path.
    #[error("settings: --settings argument is neither a file nor inline JSON: {0}")]
    CliOverlay(String),
}

/// Drive the layered load.
///
/// # Errors
/// Returns [`LoadError`] when an IO/parse failure occurs in a scope
/// file (validation errors are not fatal — they land in
/// [`LoadOutcome::validation_warnings`]).
pub fn load_settings(opts: &LoadOptions) -> Result<LoadOutcome, LoadError> {
    if opts.bare {
        return Ok(LoadOutcome {
            settings: Settings::default(),
            sources: Vec::new(),
            validation_warnings: Vec::new(),
        });
    }

    // Step 1: read each scope's raw `Value`.
    let mut per_scope: Vec<(Scope, Value, ScopeSource)> = Vec::new();
    for scope in [Scope::Managed, Scope::User, Scope::Project, Scope::Local] {
        if let Some(filter) = &opts.scope_filter
            && !filter.contains(&scope)
        {
            continue;
        }
        if let Some((value, source)) = read_scope(scope, &opts.workspace_root, &opts.paths)? {
            per_scope.push((scope, value, source));
        }
    }

    // Step 2: handle the `parent_settings_behavior: "block"` flip on
    // the managed scope.
    let managed_blocks = per_scope
        .iter()
        .find(|(scope, _, _)| *scope == Scope::Managed)
        .is_some_and(|(_, v, _)| {
            v.get("parent_settings_behavior").and_then(Value::as_str) == Some("block")
        });

    // Step 3: collect schema warnings (best-effort).
    let mut warnings = Vec::new();
    if opts.schema_validate {
        for (scope, value, _) in &per_scope {
            for err in validate_value(value) {
                warnings.push(format!("{}: {err}", scope.label()));
                tracing::warn!(target: caliban_common::tracing_targets::TARGET_SETTINGS, scope = scope.label(), error = %err, "settings schema validation warning");
            }
        }
    }

    // Step 4: build the merge order (lowest → highest).
    //
    // Default order: Managed, User, Project, Local, then Cli on top.
    // When `managed_blocks` is true: User, Project, Local, Cli, then
    // Managed on top.
    let mut order: Vec<Scope> = vec![Scope::Managed, Scope::User, Scope::Project, Scope::Local];
    if managed_blocks {
        order.retain(|s| *s != Scope::Managed);
        order.push(Scope::Managed);
    }

    let mut accumulated = Value::Object(serde_json::Map::new());
    let mut sources: Vec<ScopeSource> = Vec::new();
    for s in &order {
        if let Some((_, v, src)) = per_scope.iter().find(|(scope, _, _)| scope == s) {
            merge_values(&mut accumulated, v.clone());
            sources.push(src.clone());
        }
    }

    // Step 5: CLI overlay sits above local but below the (rare) managed-
    // block. When managed-block is active, the CLI overlay still lands
    // *before* managed so that managed's hard policy wins. Per the spec:
    // "CLI > Local > Project > User > Managed (default); when managed
    // blocks, managed jumps to top — above CLI."
    if let Some(cli) = opts.cli_overlay.clone() {
        if managed_blocks {
            // Re-construct: drop managed from accumulator, merge cli,
            // then re-apply managed on top.
            // Simpler approach: merge cli into accumulator (currently
            // ends with managed on top); managed is already on top so
            // we must put cli *below* managed. We re-do the chain
            // manually here.
            let mut redo = Value::Object(serde_json::Map::new());
            for s in &order {
                if *s == Scope::Managed {
                    continue;
                }
                if let Some((_, v, _)) = per_scope.iter().find(|(scope, _, _)| scope == s) {
                    merge_values(&mut redo, v.clone());
                }
            }
            // cli above local
            merge_values(&mut redo, cli);
            // managed on top
            if let Some((_, mv, _)) = per_scope.iter().find(|(s, _, _)| *s == Scope::Managed) {
                merge_values(&mut redo, mv.clone());
            }
            accumulated = redo;
        } else if let Some(filter) = &opts.scope_filter
            && !filter.contains(&Scope::Cli)
            && !filter.is_empty()
        {
            // CLI overlay still injected even when `--setting-sources`
            // omits it. The spec is explicit: the flag controls which
            // *scopes* are read, not the CLI overlay (which is itself a
            // CLI gesture).
            merge_values(&mut accumulated, cli);
        } else {
            merge_values(&mut accumulated, cli);
        }
        sources.push(ScopeSource {
            scope: Scope::Cli,
            path: None,
            format: Some("inline"),
        });
    }

    // Step 6: deserialize.
    let settings: Settings = serde_json::from_value(accumulated).map_err(LoadError::Final)?;
    Ok(LoadOutcome {
        settings,
        sources,
        validation_warnings: warnings,
    })
}

/// Decode `arg` from `--settings <FILE|JSON>`.
fn parse_cli_overlay(arg: &str) -> Result<Value, LoadError> {
    // Heuristic: inline JSON starts with `{` (after trim).
    let trimmed = arg.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).map_err(|e| LoadError::ParseJson {
            path: PathBuf::from("<inline-json>"),
            source: e,
        });
    }
    // Otherwise treat as path.
    let p = PathBuf::from(arg);
    let body = std::fs::read_to_string(&p).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            LoadError::CliOverlay(format!("{arg}: file not found and not inline JSON"))
        } else {
            LoadError::Io {
                path: p.clone(),
                source: e,
            }
        }
    })?;
    parse_body(&p, &body)
}

/// Read one scope: returns `Some(Value, Source)` if at least one file
/// was found and parsed; `None` if neither JSON nor TOML existed.
fn read_scope(
    scope: Scope,
    workspace_root: &Path,
    paths: &ScopePaths,
) -> Result<Option<(Value, ScopeSource)>, LoadError> {
    let Some((json_path, toml_path)) = scope.canonical_paths(workspace_root, paths) else {
        return Ok(None);
    };
    let json_exists = json_path.exists();
    let toml_exists = toml_path.exists();
    if json_exists && toml_exists {
        tracing::warn!(
            target: caliban_common::tracing_targets::TARGET_SETTINGS,
            scope = scope.label(),
            json_path = %json_path.display(),
            toml_path = %toml_path.display(),
            "both .json and .toml present in scope; .json wins"
        );
    }
    let chosen = if json_exists {
        Some((json_path.clone(), "json"))
    } else if toml_exists {
        Some((toml_path.clone(), "toml"))
    } else {
        None
    };
    let Some((path, format)) = chosen else {
        return Ok(None);
    };
    let body = std::fs::read_to_string(&path).map_err(|e| LoadError::Io {
        path: path.clone(),
        source: e,
    })?;
    let value = parse_body(&path, &body)?;
    Ok(Some((
        value,
        ScopeSource {
            scope,
            path: Some(path),
            format: Some(format),
        },
    )))
}

fn parse_body(path: &Path, body: &str) -> Result<Value, LoadError> {
    let is_toml = path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
    if is_toml {
        let toml_val: toml::Value = toml::from_str(body).map_err(|e| LoadError::ParseToml {
            path: path.to_path_buf(),
            source: e,
        })?;
        let json_val: Value = serde_json::to_value(toml_val).map_err(|e| LoadError::ParseJson {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(json_val)
    } else {
        let json_val: Value = serde_json::from_str(body).map_err(|e| LoadError::ParseJson {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(json_val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    fn fake_paths(root: &Path) -> ScopePaths {
        ScopePaths {
            managed_root: Some(root.join("managed")),
            user_config_dir: Some(root.join("user-config")),
        }
    }

    #[test]
    fn bare_mode_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let opts = LoadOptions {
            workspace_root: tmp.path().to_path_buf(),
            paths: fake_paths(tmp.path()),
            bare: true,
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        assert!(outcome.settings.model.is_none());
        assert!(outcome.sources.is_empty());
    }

    #[test]
    fn json_wins_over_toml_in_same_scope() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(
            &ws.join(".caliban/settings.json"),
            r#"{"model": "json-model"}"#,
        );
        write(
            &ws.join(".caliban/settings.toml"),
            r#"model = "toml-model""#,
        );
        let opts = LoadOptions {
            workspace_root: ws.clone(),
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        let m = outcome.settings.model.unwrap();
        assert!(matches!(m, crate::ModelSelector::Name(n) if n == "json-model"));
        let proj = outcome
            .sources
            .iter()
            .find(|s| s.scope == Scope::Project)
            .unwrap();
        assert_eq!(proj.format, Some("json"));
    }

    #[test]
    fn toml_loaded_when_json_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(&ws.join(".caliban/settings.toml"), r#"model = "from-toml""#);
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        let m = outcome.settings.model.unwrap();
        assert!(matches!(m, crate::ModelSelector::Name(n) if n == "from-toml"));
    }

    #[test]
    fn local_wins_over_project_over_user() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(
            &tmp.path().join("user-config/caliban/settings.json"),
            r#"{"model": "user", "max_tokens": 1}"#,
        );
        write(
            &ws.join(".caliban/settings.json"),
            r#"{"model": "project"}"#,
        );
        write(
            &ws.join(".caliban/settings.local.json"),
            r#"{"model": "local"}"#,
        );
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        let m = outcome.settings.model.unwrap();
        assert!(matches!(m, crate::ModelSelector::Name(n) if n == "local"));
    }

    #[test]
    fn managed_with_block_overrides_everything() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(
            &tmp.path().join("managed/managed-settings.json"),
            r#"{"model": "managed", "parent_settings_behavior": "block"}"#,
        );
        write(
            &ws.join(".caliban/settings.local.json"),
            r#"{"model": "local"}"#,
        );
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        let m = outcome.settings.model.unwrap();
        assert!(matches!(m, crate::ModelSelector::Name(n) if n == "managed"));
    }

    #[test]
    fn permission_arrays_merge_across_scopes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(
            &tmp.path().join("user-config/caliban/settings.json"),
            r#"{"permissions": {"allow": ["Read"]}}"#,
        );
        write(
            &ws.join(".caliban/settings.json"),
            r#"{"permissions": {"allow": ["Bash:git *"], "deny": ["Bash:rm *"]}}"#,
        );
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        assert_eq!(
            outcome.settings.permissions.allow,
            vec!["Read".to_string(), "Bash:git *".to_string()]
        );
        assert_eq!(
            outcome.settings.permissions.deny,
            vec!["Bash:rm *".to_string()]
        );
    }

    #[test]
    fn setting_sources_filters_scopes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(
            &tmp.path().join("user-config/caliban/settings.json"),
            r#"{"model": "user"}"#,
        );
        write(
            &ws.join(".caliban/settings.json"),
            r#"{"model": "project"}"#,
        );
        write(
            &ws.join(".caliban/settings.local.json"),
            r#"{"model": "local"}"#,
        );
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            scope_filter: Some(vec![Scope::User, Scope::Project]),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        let m = outcome.settings.model.unwrap();
        assert!(matches!(m, crate::ModelSelector::Name(n) if n == "project"));
        // Local was filtered out.
        assert!(outcome.sources.iter().all(|s| s.scope != Scope::Local));
    }

    #[test]
    fn cli_overlay_inline_json_injected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(
            &ws.join(".caliban/settings.local.json"),
            r#"{"model": "local"}"#,
        );
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            cli_overlay: Some(serde_json::json!({"model": "cli"})),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        let m = outcome.settings.model.unwrap();
        assert!(matches!(m, crate::ModelSelector::Name(n) if n == "cli"));
    }

    #[test]
    fn cli_overlay_from_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let f = tmp.path().join("overlay.json");
        fs::write(&f, r#"{"model": "from-file"}"#).unwrap();
        let opts = LoadOptions::new(tmp.path())
            .with_cli_overlay(f.to_str().unwrap())
            .unwrap();
        let outcome = load_settings(&opts).unwrap();
        let m = outcome.settings.model.unwrap();
        assert!(matches!(m, crate::ModelSelector::Name(n) if n == "from-file"));
    }

    #[test]
    fn invalid_schema_warns_but_loads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        // `parent_settings_behavior` schema allows only "block" | "augment"
        // — we feed an out-of-enum value. Type is still a string, so the
        // deserializer accepts it; schema validation emits a warning.
        write(
            &ws.join(".caliban/settings.json"),
            r#"{"permissions": {"allow": ["Read"]}, "parent_settings_behavior": "bogus-policy"}"#,
        );
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        assert!(
            !outcome.validation_warnings.is_empty(),
            "expected schema validation warning"
        );
        // Loader still succeeds with the typed value present.
        assert_eq!(outcome.settings.permissions.allow, vec!["Read".to_string()]);
        assert_eq!(
            outcome.settings.parent_settings_behavior.as_deref(),
            Some("bogus-policy")
        );
    }

    #[test]
    fn empty_directories_load_clean() {
        let tmp = tempfile::TempDir::new().unwrap();
        let opts = LoadOptions {
            workspace_root: tmp.path().to_path_buf(),
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        assert!(outcome.settings.model.is_none());
        assert!(outcome.sources.is_empty());
    }

    #[test]
    fn deep_merge_mcp_servers_across_scopes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        write(
            &tmp.path().join("user-config/caliban/settings.json"),
            r#"{"mcp_servers": {"linear": {"command": "npx", "args": ["-y"]}}}"#,
        );
        write(
            &ws.join(".caliban/settings.json"),
            r#"{"mcp_servers": {"linear": {"env": {"TOKEN": "xyz"}}}}"#,
        );
        let opts = LoadOptions {
            workspace_root: ws,
            paths: fake_paths(tmp.path()),
            ..LoadOptions::default()
        };
        let outcome = load_settings(&opts).unwrap();
        let linear = outcome.settings.mcp_servers.get("linear").unwrap();
        assert_eq!(linear.command, "npx");
        assert_eq!(linear.args, vec!["-y".to_string()]);
        assert_eq!(linear.env.get("TOKEN").unwrap(), "xyz");
    }
}
