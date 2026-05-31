//! Rule provenance: walks each active settings scope and returns its
//! permission rules tagged with the file they came from.
//!
//! Used by the `/permissions` overlay to attribute each displayed row to a
//! source file (or session, or built-in) so the `[d]` key can dispatch
//! deletion to the correct location.

use std::path::PathBuf;

use crate::loader::{LoadError, LoadOptions, load_settings};
use crate::scope::Scope;

/// Where a single rule came from. Used by the `/permissions` overlay
/// to attribute each displayed row to a file (or session, or built-in).
#[derive(Debug, Clone)]
pub struct RuleProvenance {
    /// Which scope the rule belongs to.
    pub scope: Scope,
    /// File path that produced the rule. `None` for the CLI overlay.
    pub path: Option<PathBuf>,
    /// 0-based index of the rule within that scope's permission rules vec.
    /// Used to disambiguate duplicate patterns within a single file.
    pub index_in_scope: usize,
}

/// Walk every active scope (subject to `LoadOptions::scope_filter`) and
/// return its permission rules tagged with provenance. Scope-priority
/// order is cli → local → project → user → managed (highest to lowest
/// precedence), matching the display order used by the `/permissions`
/// overlay (most-specific first).
///
/// Rules within a scope appear in source order.
///
/// # Errors
/// Returns [`LoadError`] on I/O or parse failures in any scope file.
pub fn load_rules_with_provenance(
    opts: &LoadOptions,
) -> Result<Vec<(caliban_agent_core::Rule, RuleProvenance)>, LoadError> {
    let scopes = [Scope::Local, Scope::Project, Scope::User, Scope::Managed];

    let mut out = Vec::new();

    for &scope in &scopes {
        // Respect the caller's scope_filter if any.
        if let Some(filter) = &opts.scope_filter
            && !filter.contains(&scope)
        {
            continue;
        }

        // Load only this scope.
        let mut single_opts = opts.clone();
        single_opts.scope_filter = Some(vec![scope]);
        // Disable CLI overlay so we don't double-count it.
        single_opts.cli_overlay = None;
        // Skip schema validation — we just need rules.
        single_opts.schema_validate = false;

        let outcome = load_settings(&single_opts)?;

        // Find the path that was loaded for this scope (from sources).
        let path = outcome
            .sources
            .iter()
            .find(|s| s.scope == scope)
            .and_then(|s| s.path.clone());

        // Only include this scope if it actually had a file (path is Some).
        // Scopes with no file contribute no rules.
        if path.is_none() {
            continue;
        }

        let rules = outcome.settings.permission_rules();
        for (index_in_scope, rule) in rules.into_iter().enumerate() {
            out.push((
                rule,
                RuleProvenance {
                    scope,
                    path: path.clone(),
                    index_in_scope,
                },
            ));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::ScopePaths;
    use std::fs;

    fn write(p: &std::path::Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    fn fake_paths(root: &std::path::Path) -> ScopePaths {
        ScopePaths {
            managed_root: Some(root.join("managed")),
            user_config_dir: Some(root.join("user-config")),
        }
    }

    #[test]
    fn two_scope_fixture_returns_project_before_user_with_correct_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();

        // Project scope: one rule
        write(
            &ws.join(".caliban/permissions.toml"),
            r#"
[permissions]
rules = [
  { pattern = "Bash:ls *", action = "allow" },
]
"#,
        );

        // User scope: two rules
        write(
            &tmp.path().join("user-config/caliban/permissions.toml"),
            r#"
[permissions]
rules = [
  { pattern = "Read", action = "allow" },
  { pattern = "Write", action = "ask" },
]
"#,
        );

        let opts = LoadOptions {
            workspace_root: ws.clone(),
            paths: fake_paths(tmp.path()),
            scope_filter: Some(vec![Scope::Project, Scope::User]),
            cli_overlay: None,
            bare: false,
            schema_validate: false,
        };

        let result = load_rules_with_provenance(&opts).unwrap();

        // Expect: Local first (skipped — no file), Project next (1 rule), User last (2 rules)
        assert_eq!(result.len(), 3, "expected 3 rules total");

        // First rule should be from Project scope
        let (rule0, prov0) = &result[0];
        assert_eq!(rule0.tool, "Bash:ls *");
        assert_eq!(prov0.scope, Scope::Project);
        assert_eq!(prov0.index_in_scope, 0);
        let proj_path = prov0.path.as_ref().expect("project path must be Some");
        assert!(
            proj_path.starts_with(&ws),
            "project path {proj_path:?} should be under workspace {ws:?}"
        );

        // Second rule should be from User scope
        let (rule1, prov1) = &result[1];
        assert_eq!(rule1.tool, "Read");
        assert_eq!(prov1.scope, Scope::User);
        assert_eq!(prov1.index_in_scope, 0);
        let user_path = prov1.path.as_ref().expect("user path must be Some");
        assert!(
            user_path.starts_with(tmp.path()),
            "user path {user_path:?} should be under tmp {tmp:?}",
            tmp = tmp.path()
        );

        // Third rule from User scope
        let (rule2, prov2) = &result[2];
        assert_eq!(rule2.tool, "Write");
        assert_eq!(prov2.scope, Scope::User);
        assert_eq!(prov2.index_in_scope, 1);
        assert_eq!(
            prov2.path, prov1.path,
            "both user rules share the same path"
        );
    }
}
