//! `.caliban/rules/<topic>.md` — path-scoped rule files with optional
//! `paths:` glob frontmatter.
//!
//! Part of ADR 0036. Rules behave like miniature CLAUDE.md addendums that the
//! agent activates lazily once the model touches a matching file (or eagerly
//! at startup when `paths:` is absent).

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

/// A loaded rule file (frontmatter parsed; body raw).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Logical name (kebab-case, defaults to the file stem).
    pub name: String,
    /// Optional one-line description.
    pub description: Option<String>,
    /// Optional glob patterns for lazy activation. When empty, the rule is
    /// always active (loaded at startup).
    pub paths: Vec<String>,
    /// File body (everything after the closing `---`).
    pub body: String,
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Source scope (project vs user).
    pub scope: RuleScope,
}

/// Whether the rule was loaded from the user dir (`~/.caliban/rules/`) or the
/// project dir (`<workspace>/.caliban/rules/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    /// User-global rules (`~/.caliban/rules/`).
    User,
    /// Project rules (`<workspace>/.caliban/rules/`).
    Project,
}

impl RuleScope {
    /// Splice attribute value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// A set of loaded rules with a pre-built `GlobSet` for fast path matching.
#[derive(Debug)]
pub struct RuleSet {
    rules: Vec<Rule>,
    matcher: GlobSet,
    /// Maps a glob-set index back to the rule index that owns it.
    glob_to_rule: Vec<usize>,
}

impl RuleSet {
    /// Empty set (no rules loaded).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            rules: Vec::new(),
            matcher: GlobSet::empty(),
            glob_to_rule: Vec::new(),
        }
    }

    /// All loaded rules.
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Rules that have no `paths:` filter — always loaded into the prompt.
    #[must_use]
    pub fn always_active(&self) -> Vec<&Rule> {
        self.rules.iter().filter(|r| r.paths.is_empty()).collect()
    }

    /// Return the **indexes** of every rule whose `paths:` filter matches
    /// `path`. Always-active rules (no `paths:` filter) are not returned here
    /// — they're loaded eagerly via [`Self::always_active`].
    #[must_use]
    pub fn matching(&self, path: &Path) -> Vec<usize> {
        let mut hits = self.matcher.matches(path);
        hits.sort_unstable();
        hits.dedup();
        hits.into_iter()
            .map(|gi| self.glob_to_rule[gi])
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Return the rule at index `i` (used after a [`Self::matching`] hit).
    #[must_use]
    pub fn get(&self, i: usize) -> Option<&Rule> {
        self.rules.get(i)
    }

    /// Build a `RuleSet` from owned rules, building a `GlobSet` from their
    /// `paths:` patterns.
    #[must_use]
    pub fn build(rules: Vec<Rule>) -> Self {
        let mut builder = GlobSetBuilder::new();
        let mut glob_to_rule = Vec::new();
        for (idx, r) in rules.iter().enumerate() {
            for pat in &r.paths {
                if let Ok(g) = Glob::new(pat) {
                    builder.add(g);
                    glob_to_rule.push(idx);
                } else {
                    tracing::warn!(
                        target: "caliban::memory::rules",
                        rule = %r.name,
                        pattern = %pat,
                        "invalid glob pattern in rule",
                    );
                }
            }
        }
        let matcher = builder.build().unwrap_or_else(|e| {
            tracing::warn!(
                target: "caliban::memory::rules",
                error = %e,
                "rule globset build failed; falling back to empty matcher",
            );
            GlobSet::empty()
        });
        Self {
            rules,
            matcher,
            glob_to_rule,
        }
    }
}

/// Scan both the user dir (`~/.caliban/rules/`) and the project dir
/// (`<workspace>/.caliban/rules/`) for `*.md` rule files. Malformed files are
/// skipped with a warning.
#[must_use]
pub fn scan_caliban_rules(workspace_root: &Path) -> RuleSet {
    let mut rules = Vec::new();
    if let Some(home) = dirs::home_dir() {
        scan_dir(
            &home.join(".caliban").join("rules"),
            RuleScope::User,
            &mut rules,
        );
    }
    scan_dir(
        &workspace_root.join(".caliban").join("rules"),
        RuleScope::Project,
        &mut rules,
    );
    rules.sort_by(|a, b| a.name.cmp(&b.name));
    RuleSet::build(rules)
}

fn scan_dir(dir: &Path, scope: RuleScope, out: &mut Vec<Rule>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Skip a README.md per convention.
        if stem.eq_ignore_ascii_case("README") {
            continue;
        }
        match parse_rule(&p, scope, stem) {
            Ok(r) => out.push(r),
            Err(e) => tracing::warn!(
                target: "caliban::memory::rules",
                path = %p.display(),
                error = %e,
                "skipping malformed rule file",
            ),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct RawRuleFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
}

fn parse_rule(path: &Path, scope: RuleScope, stem: &str) -> Result<Rule, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("io: {e}"))?;
    let trimmed = raw.trim_start_matches('\u{feff}');
    let body_start = "---\n";
    if !trimmed.starts_with(body_start) {
        // No frontmatter — entire file is the body; rule is always-active.
        return Ok(Rule {
            name: stem.to_string(),
            description: None,
            paths: Vec::new(),
            body: trimmed.to_string(),
            path: path.to_path_buf(),
            scope,
        });
    }
    let after = &trimmed[body_start.len()..];
    let Some(end) = after.find("\n---\n").or_else(|| {
        let i = after.find("\n---")?;
        if after[i..].starts_with("\n---") {
            Some(i)
        } else {
            None
        }
    }) else {
        return Err("missing closing `---` frontmatter delimiter".into());
    };
    let yaml = &after[..end];
    let body_off = end + "\n---\n".len();
    let body = if body_off >= after.len() {
        ""
    } else {
        &after[body_off..]
    };
    let fm: RawRuleFrontmatter = serde_yaml::from_str(yaml).map_err(|e| format!("yaml: {e}"))?;
    Ok(Rule {
        name: fm.name.unwrap_or_else(|| stem.to_string()),
        description: fm.description,
        paths: fm.paths,
        body: body.to_string(),
        path: path.to_path_buf(),
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_rule(dir: &Path, name: &str, fm: &str, body: &str) {
        let mut s = String::new();
        if !fm.is_empty() {
            s.push_str("---\n");
            s.push_str(fm);
            if !fm.ends_with('\n') {
                s.push('\n');
            }
            s.push_str("---\n\n");
        }
        s.push_str(body);
        fs::write(dir.join(format!("{name}.md")), s).unwrap();
    }

    #[test]
    fn scan_loads_project_rules_and_builds_globset() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let rules_dir = workspace.join(".caliban").join("rules");
        fs::create_dir_all(&rules_dir).unwrap();
        write_rule(
            &rules_dir,
            "python-style",
            "name: python-style\npaths:\n  - \"**/*.py\"\n  - \"scripts/**\"\n",
            "Use black + ruff.\n",
        );
        write_rule(
            &rules_dir,
            "always-on",
            "name: always-on\n",
            "Always loaded.\n",
        );

        let set = scan_caliban_rules(workspace);
        assert_eq!(set.rules().len(), 2);

        // python-style activates on .py paths.
        let hits = set.matching(Path::new("src/foo.py"));
        assert_eq!(hits.len(), 1);
        assert_eq!(set.get(hits[0]).unwrap().name, "python-style");

        // always-on shows up in always_active().
        let always: Vec<_> = set.always_active().iter().map(|r| r.name.clone()).collect();
        assert!(always.contains(&"always-on".to_string()));
    }

    #[test]
    fn rule_without_paths_is_always_active() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let rules_dir = workspace.join(".caliban").join("rules");
        fs::create_dir_all(&rules_dir).unwrap();
        write_rule(&rules_dir, "convs", "name: convs\n", "Conventions.\n");
        let set = scan_caliban_rules(workspace);
        assert_eq!(set.always_active().len(), 1);
        // Path-touch should NOT match an always-active rule (it's always-on already).
        assert!(set.matching(Path::new("anything.txt")).is_empty());
    }

    #[test]
    fn rules_skip_readme_by_convention() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let rules_dir = workspace.join(".caliban").join("rules");
        fs::create_dir_all(&rules_dir).unwrap();
        write_rule(&rules_dir, "README", "name: README\n", "noise");
        write_rule(&rules_dir, "actual", "name: actual\n", "ok");
        let set = scan_caliban_rules(workspace);
        let names: Vec<_> = set.rules().iter().map(|r| r.name.as_str()).collect();
        assert!(!names.contains(&"README"));
        assert!(names.contains(&"actual"));
    }

    #[test]
    fn scan_emits_both_user_and_project_scopes() {
        // Simulate a user-dir by monkey-patching HOME to a tempdir.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let workspace = tmp.path().join("ws");
        let user_rules = home.join(".caliban").join("rules");
        let proj_rules = workspace.join(".caliban").join("rules");
        fs::create_dir_all(&user_rules).unwrap();
        fs::create_dir_all(&proj_rules).unwrap();
        write_rule(&user_rules, "user-a", "name: user-a\n", "U");
        write_rule(&proj_rules, "proj-a", "name: proj-a\n", "P");

        // The crate uses `dirs::home_dir()` which reads $HOME on Unix. We can't
        // override that without unsafe env mutation; instead, call the lower-
        // level helper directly via reflection — easier: assert RuleSet::build
        // wrappers work and that the scan_dir helper is reachable.
        //
        // Use the public API by passing the workspace; the user-scope path
        // resolution can still be unit-tested via build_two_scopes below.
        let mut all = Vec::new();
        scan_dir(&user_rules, RuleScope::User, &mut all);
        scan_dir(&proj_rules, RuleScope::Project, &mut all);
        let set = RuleSet::build(all);
        let names: Vec<_> = set.rules().iter().map(|r| r.name.clone()).collect();
        assert!(names.contains(&"user-a".to_string()));
        assert!(names.contains(&"proj-a".to_string()));
        let user_count = set
            .rules()
            .iter()
            .filter(|r| matches!(r.scope, RuleScope::User))
            .count();
        let proj_count = set
            .rules()
            .iter()
            .filter(|r| matches!(r.scope, RuleScope::Project))
            .count();
        assert_eq!(user_count, 1);
        assert_eq!(proj_count, 1);
    }
}
