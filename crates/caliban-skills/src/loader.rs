//! Filesystem walker + frontmatter parser for `SKILL.md` files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::skill::{Frontmatter, Skill};

/// Discovery roots checked in priority order. The first match for a given
/// `name` wins; later roots are shadowed.
#[must_use]
pub fn default_roots(workspace_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(3);
    out.push(workspace_root.join(".caliban").join("skills"));
    if let Some(d) = caliban_common::paths::platform_config_dir() {
        out.push(d.join("caliban").join("skills"));
    }
    if let Some(d) = caliban_common::paths::platform_data_dir() {
        out.push(d.join("caliban").join("plugins"));
    }
    out
}

/// A `SKILL.md` that was discovered on disk but rejected, paired with the
/// reason. Surfaced to users (startup stderr + `caliban doctor`) so a misnamed
/// or malformed skill does not vanish silently — see issue #107.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSkip {
    /// Path to the rejected `SKILL.md`.
    pub path: PathBuf,
    /// Human-readable reason (name/dir mismatch, bad frontmatter, empty
    /// description, …) — the error string from [`load_one`].
    pub reason: String,
}

/// Outcome of a skill scan: the skills that loaded, plus any discovered-but-
/// rejected files. `skips` excludes intentionally *shadowed* duplicates (a
/// later root losing to an earlier one is expected, not a loss).
#[derive(Debug, Clone, Default)]
pub struct SkillLoadReport {
    /// Successfully loaded skills, sorted by name.
    pub skills: Vec<Skill>,
    /// Rejected files, sorted by path.
    pub skips: Vec<SkillSkip>,
}

/// Load all skills from the given roots in priority order, returning both the
/// loaded skills and any discovered-but-rejected files.
///
/// Missing roots are silently skipped. Rejected `SKILL.md` files are logged at
/// `warn!` *and* recorded in [`SkillLoadReport::skips`] so callers can surface
/// them to the user — loading itself remains best-effort.
#[must_use]
pub fn load_skills_report(roots: &[PathBuf]) -> SkillLoadReport {
    let mut by_name: HashMap<String, Skill> = HashMap::new();
    let mut skips: Vec<SkillSkip> = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        for entry in ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(false)
            .build()
            .flatten()
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) != Some("SKILL.md") {
                continue;
            }
            match load_one(path) {
                Ok(skill) => {
                    if by_name.contains_key(&skill.name) {
                        tracing::debug!(
                            target: caliban_common::tracing_targets::TARGET_SKILLS,
                            name = %skill.name,
                            path = %skill.source_path.display(),
                            "skipping shadowed skill (already loaded from earlier root)",
                        );
                    } else {
                        by_name.insert(skill.name.clone(), skill);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_SKILLS,
                        path = %path.display(),
                        error = %e,
                        "skipping malformed skill",
                    );
                    skips.push(SkillSkip {
                        path: path.to_path_buf(),
                        reason: e,
                    });
                }
            }
        }
    }

    let mut skills: Vec<Skill> = by_name.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skips.sort_by(|a, b| a.path.cmp(&b.path));
    SkillLoadReport { skills, skips }
}

/// Load all skills from the given roots in priority order.
///
/// Thin wrapper over [`load_skills_report`] that drops the skip report. Missing
/// roots are silently skipped; rejected `SKILL.md` files are logged at `warn!`
/// and skipped — loading is best-effort.
#[must_use]
pub fn load_skills(roots: &[PathBuf]) -> Vec<Skill> {
    load_skills_report(roots).skills
}

/// Parse a single `SKILL.md` file. Returns an error on missing frontmatter,
/// mismatched name vs parent directory, or empty description.
///
/// # Errors
/// Returns a string description of the parse failure.
pub fn load_one(path: &Path) -> Result<Skill, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("io: {e}"))?;
    let (yaml_chunk, body) =
        caliban_common::frontmatter::split(&raw).map_err(|e| e.reason().to_string())?;
    let body = body.to_string();

    let fm: Frontmatter =
        serde_yaml::from_str(yaml_chunk).map_err(|e| format!("invalid frontmatter yaml: {e}"))?;

    if fm.description.trim().is_empty() {
        return Err("description must be non-empty".into());
    }

    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .ok_or_else(|| "could not derive parent directory name".to_string())?;
    if parent_name != fm.name {
        return Err(format!(
            "skill name '{}' does not match parent directory '{}'",
            fm.name, parent_name
        ));
    }

    Ok(Skill {
        name: fm.name,
        description: fm.description,
        body,
        metadata: fm.metadata,
        source_path: path.to_path_buf(),
    })
}
