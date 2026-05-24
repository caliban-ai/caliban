//! Built-in skills bundled with the caliban harness.
//!
//! These are compiled into the binary via [`include_str!`] so they ship
//! without any on-disk install step. They register *before* the user-dir
//! skill scan so a user-supplied skill with the same name will shadow the
//! built-in (matching the resolution rule already used by [`crate::load_skills`]).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::skill::Skill;

/// Embedded body of the built-in `auto-memory` skill. See
/// `docs/superpowers/specs/2026-05-24-auto-memory-design.md` for the protocol
/// this body documents.
const AUTO_MEMORY_SKILL_MD: &str = include_str!("builtins/auto_memory.md");

/// Frontmatter shape used to parse the embedded skill files.
#[derive(Debug, Deserialize)]
struct EmbeddedFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    metadata: BTreeMap<String, serde_yaml::Value>,
}

/// Parse one embedded SKILL.md into a [`Skill`]. Panics on failure — the
/// embedded bodies ship with the binary, so failure indicates a programmer
/// error (malformed frontmatter), not a runtime/operator issue.
fn parse_embedded(name_hint: &'static str, raw: &str) -> Skill {
    let trimmed = raw.trim_start_matches('\u{feff}');
    let opener = "---\n";
    assert!(
        trimmed.starts_with(opener),
        "builtin skill {name_hint} missing leading frontmatter delimiter"
    );
    let after_start = &trimmed[opener.len()..];
    let end_idx = after_start
        .find("\n---\n")
        .or_else(|| {
            after_start
                .find("\n---")
                .filter(|i| after_start[*i..].starts_with("\n---"))
        })
        .unwrap_or_else(|| {
            panic!("builtin skill {name_hint} missing closing frontmatter delimiter")
        });
    let yaml_chunk = &after_start[..end_idx];
    let body_start_offset = end_idx + "\n---\n".len();
    let body = if body_start_offset >= after_start.len() {
        String::new()
    } else {
        after_start[body_start_offset..].to_string()
    };
    let fm: EmbeddedFrontmatter = serde_yaml::from_str(yaml_chunk)
        .unwrap_or_else(|e| panic!("builtin skill {name_hint} bad yaml: {e}"));
    Skill {
        name: fm.name,
        description: fm.description,
        body,
        metadata: fm.metadata,
        source_path: PathBuf::from(format!("<builtin:{name_hint}>")),
    }
}

/// Return every built-in skill, ready to be merged into the regular skill
/// list before user-dir scanning.
#[must_use]
pub fn builtin_skills() -> Vec<Skill> {
    vec![parse_embedded("auto-memory", AUTO_MEMORY_SKILL_MD)]
}

/// Register every built-in skill into `dest`. Skills already present in
/// `dest` (by name) are *not* overwritten — the built-in is treated as a
/// fallback that user-dir scans can shadow.
pub fn register(dest: &mut Vec<Skill>) {
    let existing: std::collections::HashSet<String> = dest.iter().map(|s| s.name.clone()).collect();
    for s in builtin_skills() {
        if existing.contains(&s.name) {
            continue;
        }
        dest.push(s);
    }
    dest.sort_by(|a, b| a.name.cmp(&b.name));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_memory_skill_parses_with_expected_fields() {
        let skills = builtin_skills();
        let s = skills
            .iter()
            .find(|s| s.name == "auto-memory")
            .expect("auto-memory skill present");
        assert!(!s.description.trim().is_empty());
        assert!(s.body.contains("When to READ"));
        assert!(s.body.contains("When to WRITE"));
        assert!(s.body.contains("DO NOT save"));
        // disable_model_invocation: false comes through as a metadata field on
        // the skill (the parser leaves anything outside name/description in
        // `metadata`); we just sanity-check the body talks about the four
        // memory types here.
        for kind in ["user", "feedback", "project", "reference"] {
            assert!(s.body.contains(kind), "missing kind {kind}");
        }
    }

    #[test]
    fn register_appends_builtin_when_absent() {
        let mut dest: Vec<Skill> = Vec::new();
        register(&mut dest);
        assert!(dest.iter().any(|s| s.name == "auto-memory"));
    }

    #[test]
    fn register_does_not_shadow_existing_skill() {
        // Pretend a user-dir skill named "auto-memory" is already loaded.
        let mut dest = vec![Skill {
            name: "auto-memory".into(),
            description: "user override".into(),
            body: "user body".into(),
            metadata: BTreeMap::new(),
            source_path: PathBuf::from("/some/user/skills/auto-memory/SKILL.md"),
        }];
        register(&mut dest);
        let s = dest.iter().find(|s| s.name == "auto-memory").unwrap();
        assert_eq!(s.description, "user override");
        assert_eq!(s.body, "user body");
    }
}
