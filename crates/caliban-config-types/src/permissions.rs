//! Permission rule data types + the legacy `permissions.toml` loader.
//!
//! The rule evaluation engine, runtime rule store, and `PermissionsHook` stay
//! in `caliban-agent-core` (they depend on its runtime `ToolCtx`/`Hooks`). Only
//! the plain data types and the dependency-free loader live here, so
//! `caliban-settings` can consume them without inverting the layering (epic
//! #539 / ADR 0061). See `docs/adr/0020-permission-rules.md`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The outcome of matching a rule against a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Run the tool without prompting.
    Allow,
    /// Reject the tool call.
    Deny,
    /// Defer to an interactive prompt (resolved by the permissions layer's
    /// ask handler in `caliban-agent-core`).
    Ask,
}

/// One rule from a TOML file or CLI flag.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rule {
    /// Pattern of the form `Tool` or `Tool:first-arg-glob`.
    pub tool: String,
    /// Action to take when the pattern matches.
    pub action: Action,
    /// Optional comment displayed in the Ask modal + audit log; never seen by the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Deny-only; surfaces to the model in place of the generic
    /// "permission denied" message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Reserved for v3 time-bounded rules; v2 parses but ignores at evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

/// Built-in default rules applied at the lowest priority. Read-only tools
/// Allow; mutating tools Ask; catch-all is Ask.
#[must_use]
pub fn default_rules() -> Vec<Rule> {
    [
        ("Read", Action::Allow),
        ("Grep", Action::Allow),
        ("Glob", Action::Allow),
        ("WebFetch", Action::Ask),
        ("Bash", Action::Ask),
        ("Write", Action::Ask),
        ("Edit", Action::Ask),
        ("TodoWrite", Action::Allow),
        ("EnterPlanMode", Action::Allow),
        ("ExitPlanMode", Action::Allow),
        ("*", Action::Ask),
    ]
    .into_iter()
    .map(|(t, a)| Rule {
        tool: t.into(),
        action: a,
        comment: None,
        reason: None,
        expires_at: None,
    })
    .collect()
}

/// Errors emitted by the permissions loader.
#[derive(thiserror::Error, Debug)]
pub enum PermissionsLoadError {
    /// IO failure reading a permissions file.
    #[error("permissions: io error reading {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse error.
    #[error("permissions: parse error in {path}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
}

/// Load rules from a TOML file. Missing file → `Ok(vec![])`.
///
/// # Errors
/// Returns [`PermissionsLoadError::Io`] on read errors other than `NotFound`,
/// and [`PermissionsLoadError::Parse`] on malformed TOML.
#[deprecated(
    since = "0.0.1",
    note = "load via caliban-settings; legacy loaders remove in v0.2"
)]
pub fn load_rules_file(path: &Path) -> std::result::Result<Vec<Rule>, PermissionsLoadError> {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(PermissionsLoadError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };
    let parsed: RulesFile =
        toml::from_str(&body).map_err(|source| PermissionsLoadError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(parsed.rules)
}

/// Resolve and load rules from the standard locations:
/// 1. CLI rules (highest priority — caller-supplied).
/// 2. Project file `<workspace>/.caliban/permissions.toml`.
/// 3. User file `$XDG_CONFIG_HOME/caliban/permissions.toml`.
/// 4. Built-in defaults.
///
/// Rules from higher-priority sources are placed first; first-match-wins
/// at evaluation time.
///
/// # Errors
/// Propagates [`PermissionsLoadError`] from the project or user file readers.
#[deprecated(
    since = "0.0.1",
    note = "load via caliban-settings; legacy loaders remove in v0.2"
)]
pub fn load_rules(
    cli_rules: Vec<Rule>,
    workspace_root: &Path,
) -> std::result::Result<Vec<Rule>, PermissionsLoadError> {
    let mut all = cli_rules;

    let project_file = workspace_root.join(".caliban/permissions.toml");
    #[allow(deprecated)]
    all.extend(load_rules_file(&project_file)?);

    let user_dir = caliban_common::paths::platform_config_dir()
        .map(|d| d.join("caliban").join("permissions.toml"));
    if let Some(p) = user_dir {
        #[allow(deprecated)]
        all.extend(load_rules_file(&p)?);
    }

    all.extend(default_rules());
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_serde_is_lowercase() {
        let r: Rule = toml::from_str("tool = \"Bash\"\naction = \"deny\"").unwrap();
        assert_eq!(r.tool, "Bash");
        assert_eq!(r.action, Action::Deny);
    }

    #[test]
    fn default_rules_cover_core_tools_and_catch_all() {
        let rules = default_rules();
        assert_eq!(rules.last().unwrap().tool, "*");
        assert!(
            rules
                .iter()
                .any(|r| r.tool == "Read" && r.action == Action::Allow)
        );
    }

    #[test]
    #[allow(deprecated)]
    fn load_rules_file_missing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        assert!(load_rules_file(&missing).unwrap().is_empty());
    }

    #[test]
    #[allow(deprecated)]
    fn load_rules_file_parses_rule_table() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("permissions.toml");
        std::fs::write(&f, "[[rule]]\ntool = \"Write\"\naction = \"allow\"\n").unwrap();
        let rules = load_rules_file(&f).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action, Action::Allow);
    }

    #[test]
    #[allow(deprecated)]
    fn rule_deserializes_reason_and_expires_at() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("permissions.toml");
        std::fs::write(
            &f,
            "[[rule]]\ntool = \"Bash\"\naction = \"deny\"\n\
             reason = \"no shell access in CI\"\nexpires_at = \"2026-12-31T00:00:00Z\"\n",
        )
        .unwrap();
        let rules = load_rules_file(&f).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action, Action::Deny);
        assert_eq!(rules[0].reason.as_deref(), Some("no shell access in CI"));
        assert!(rules[0].expires_at.is_some());
    }
}
