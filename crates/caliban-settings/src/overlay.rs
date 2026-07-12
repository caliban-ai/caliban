//! Read-only rendering of the merged settings + scope provenance.
//!
//! Used by both the `/config` slash overlay (TUI) and the future
//! `caliban config print` subcommand. The full read-write editor lands
//! with ADR 0040 (slash-command registry).

use crate::{LoadOutcome, Settings};

/// One row in the rendered `/config` view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRow {
    /// Top-level key (e.g. `"model"`, `"permissions.allow"`).
    pub key: String,
    /// Stringified effective value.
    pub value: String,
    /// Provenance — which scope contributed the value. `None` means
    /// "not set by any scope".
    pub scope: Option<String>,
}

/// Render the merged settings into a list of `(key, value, scope)`
/// rows. Per-scope provenance is currently a best-effort lookup; deep
/// provenance per nested key lands with ADR 0040.
#[must_use]
pub fn render_rows(outcome: &LoadOutcome) -> Vec<ConfigRow> {
    let settings = &outcome.settings;
    let mut out = Vec::new();

    macro_rules! row {
        ($key:expr, $val:expr) => {
            out.push(ConfigRow {
                key: $key.to_string(),
                value: $val,
                scope: scope_for(outcome, $key),
            });
        };
    }

    if let Some(m) = settings.model.as_ref() {
        row!("model", m.display());
    }
    if let Some(m) = settings.fallback_model.as_ref() {
        row!("fallback_model", m.display());
    }
    if let Some(a) = settings.agent.as_ref() {
        row!("agent", a.clone());
    }
    if let Some(b) = settings.enable_telemetry {
        row!("enable_telemetry", b.to_string());
    }
    if let Some(p) = settings.parent_settings_behavior.as_ref() {
        row!("parent_settings_behavior", p.clone());
    }
    if !settings.permissions.allow.is_empty() {
        row!(
            "permissions.allow",
            format!("[{}]", settings.permissions.allow.join(", "))
        );
    }
    if !settings.permissions.ask.is_empty() {
        row!(
            "permissions.ask",
            format!("[{}]", settings.permissions.ask.join(", "))
        );
    }
    if !settings.permissions.deny.is_empty() {
        row!(
            "permissions.deny",
            format!("[{}]", settings.permissions.deny.join(", "))
        );
    }
    if !settings.mcp_servers.is_empty() {
        let names: Vec<&str> = settings.mcp_servers.keys().map(String::as_str).collect();
        row!("mcp_servers", format!("[{}]", names.join(", ")));
    }
    if let Some(s) = settings.output_style.as_ref() {
        row!("output_style", s.clone());
    }
    if let Some(s) = settings.editor_mode.as_ref() {
        row!("editor_mode", s.clone());
    }
    if let Some(s) = settings.view_mode.as_ref() {
        row!("view_mode", s.clone());
    }
    if settings.api_key_helper.is_some() {
        row!("api_key_helper", "<configured>".to_string());
    }
    out
}

/// Return the scope label (`"managed"`, `"user"`, `"project"`, …) that
/// actually set `key` — the highest-precedence scope that declared its
/// top-level segment, per [`LoadOutcome::provenance`] (#411).
///
/// Attribution is at top-level-key granularity: `permissions.allow` is
/// attributed to whichever scope declared `permissions`. Deep per-nested-key
/// provenance lands with ADR 0040.
fn scope_for(outcome: &LoadOutcome, key: &str) -> Option<String> {
    let top = key.split('.').next().unwrap_or(key);
    outcome
        .provenance
        .get(top)
        .map(|scope| scope.label().to_string())
}

/// Render to plain text for `caliban config print`.
///
/// # Panics
/// Panics only if `write!` to an in-memory `String` fails, which
/// should never happen.
#[must_use]
pub fn render_text(outcome: &LoadOutcome) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let rows = render_rows(outcome);
    let max_key = rows.iter().map(|r| r.key.len()).max().unwrap_or(0);
    for r in rows {
        let scope = r.scope.as_deref().unwrap_or("?");
        let _ = writeln!(
            &mut out,
            "{:width$}  {}  [{}]",
            r.key,
            r.value,
            scope,
            width = max_key
        );
    }
    out
}

/// Lookup a `Settings` field by its dotted JSON pointer. Used by tests
/// and by `caliban config get <key>`.
#[must_use]
pub fn get(settings: &Settings, key: &str) -> Option<serde_json::Value> {
    let v = serde_json::to_value(settings).ok()?;
    let mut cur = &v;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LoadOptions, Permissions};

    fn build_outcome() -> LoadOutcome {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        let user_dir = tmp.path().join("user-config");
        std::fs::create_dir_all(user_dir.join("caliban")).unwrap();
        std::fs::write(
            user_dir.join("caliban/settings.json"),
            r#"{"model": "user-m", "permissions": {"allow": ["Read"]}}"#,
        )
        .unwrap();
        let opts = LoadOptions {
            workspace_root: ws,
            paths: crate::ScopePaths {
                managed_root: None,
                user_config_dir: Some(user_dir),
            },
            ..LoadOptions::default()
        };
        crate::load_settings(&opts).unwrap()
    }

    #[test]
    fn rows_include_model_and_permissions() {
        let outcome = build_outcome();
        let rows = render_rows(&outcome);
        assert!(rows.iter().any(|r| r.key == "model"));
        assert!(rows.iter().any(|r| r.key == "permissions.allow"));
    }

    #[test]
    fn render_text_contains_keys() {
        let outcome = build_outcome();
        let text = render_text(&outcome);
        assert!(text.contains("model"));
        assert!(text.contains("permissions.allow"));
        assert!(text.contains("[user]"));
    }

    #[test]
    fn get_returns_nested_value() {
        let s = Settings {
            permissions: Permissions {
                allow: vec!["Read".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let v = get(&s, "permissions.allow").unwrap();
        assert!(v.as_array().is_some_and(|a| a.len() == 1));
    }

    #[test]
    fn scope_for_reports_declaring_scope() {
        let outcome = build_outcome();
        // `model` is set only in the user scope.
        assert_eq!(scope_for(&outcome, "model").as_deref(), Some("user"));
        // Nested keys resolve to their top-level segment's scope.
        assert_eq!(
            scope_for(&outcome, "permissions.allow").as_deref(),
            Some("user")
        );
    }

    #[test]
    fn scope_for_attributes_each_key_to_its_true_scope() {
        // #411 acceptance: a value set in a *lower* scope while a *higher* scope
        // contributes other keys must be attributed to the scope that set it,
        // not merely the top contributing scope.
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        // User scope (lower) sets `model`.
        let user_dir = tmp.path().join("user-config");
        std::fs::create_dir_all(user_dir.join("caliban")).unwrap();
        std::fs::write(
            user_dir.join("caliban/settings.json"),
            r#"{"model": "user-m"}"#,
        )
        .unwrap();
        // Project scope (higher) sets a *different* key, `agent`.
        std::fs::create_dir_all(ws.join(".caliban")).unwrap();
        std::fs::write(ws.join(".caliban/settings.json"), r#"{"agent": "proj-a"}"#).unwrap();

        let opts = LoadOptions {
            workspace_root: ws,
            paths: crate::ScopePaths {
                managed_root: None,
                user_config_dir: Some(user_dir),
            },
            ..LoadOptions::default()
        };
        let outcome = crate::load_settings(&opts).unwrap();

        // The bug: `scope_for` used to return `sources.last()` (project) for
        // every key. It must now attribute each key to its real source.
        assert_eq!(
            scope_for(&outcome, "model").as_deref(),
            Some("user"),
            "model was set in the user scope, not project"
        );
        assert_eq!(
            scope_for(&outcome, "agent").as_deref(),
            Some("project"),
            "agent was set in the project scope"
        );
    }
}
