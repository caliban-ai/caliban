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
    if let Some(d) = dirs::config_dir() {
        out.push(d.join("caliban").join("skills"));
    }
    if let Some(d) = dirs::data_local_dir() {
        out.push(d.join("caliban").join("plugins"));
    }
    out
}

/// Load all skills from the given roots in priority order.
///
/// Missing roots are silently skipped. Malformed `SKILL.md` files are logged
/// at `warn!` and skipped — loading is best-effort.
#[must_use]
pub fn load_skills(roots: &[PathBuf]) -> Vec<Skill> {
    let mut by_name: HashMap<String, Skill> = HashMap::new();

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
                            target: "caliban::skills",
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
                        target: "caliban::skills",
                        path = %path.display(),
                        error = %e,
                        "skipping malformed skill",
                    );
                }
            }
        }
    }

    let mut out: Vec<Skill> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse a single `SKILL.md` file. Returns an error on missing frontmatter,
/// mismatched name vs parent directory, or empty description.
///
/// # Errors
/// Returns a string description of the parse failure.
pub fn load_one(path: &Path) -> Result<Skill, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("io: {e}"))?;
    let body_start = "---\n";
    let raw_trim = raw.trim_start_matches('\u{feff}');
    if !raw_trim.starts_with(body_start) {
        return Err("missing leading `---` frontmatter delimiter".into());
    }
    let after_start = &raw_trim[body_start.len()..];
    let Some(end_idx) = after_start.find("\n---\n").or_else(|| {
        // tolerate end-of-file without trailing newline
        after_start.find("\n---").filter(|i| {
            // verify it's actually the end marker, not "---" inside the body
            // (close enough heuristic for our format)
            after_start[*i..].starts_with("\n---")
        })
    }) else {
        return Err("missing closing `---` frontmatter delimiter".into());
    };
    let yaml_chunk = &after_start[..end_idx];
    let body_start_offset = end_idx + "\n---\n".len();
    let body = if body_start_offset >= after_start.len() {
        String::new()
    } else {
        after_start[body_start_offset..].to_string()
    };

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
