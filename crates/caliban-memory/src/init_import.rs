//! `/init` companion: probe a workspace for legacy / sibling-tool guidance
//! files (`AGENTS.md`, `.cursorrules`, `.windsurfrules`) and concatenate their
//! contents into a single body the operator can paste into their CLAUDE.md.
//!
//! Part of ADR 0036 — see the spec's "AGENTS.md" + `/init` notes.

use std::path::{Path, PathBuf};

/// One legacy-rules file discovered in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRulesFile {
    /// Filename (e.g. `AGENTS.md`).
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
    /// File body (UTF-8 lossy).
    pub body: String,
}

/// Filenames probed by `/init` in order of precedence.
pub const INIT_FILENAMES: &[&str] = &["AGENTS.md", ".cursorrules", ".windsurfrules"];

/// Scan `workspace_root` for any of [`INIT_FILENAMES`] and return the ones
/// that exist as a `Vec<LegacyRulesFile>` in declaration order.
#[must_use]
pub fn scan_init_files(workspace_root: &Path) -> Vec<LegacyRulesFile> {
    let mut out = Vec::new();
    for name in INIT_FILENAMES {
        let path = workspace_root.join(name);
        if !path.is_file() {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => out.push(LegacyRulesFile {
                name: (*name).to_string(),
                path,
                body: String::from_utf8_lossy(&bytes).into_owned(),
            }),
            Err(e) => tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_MEMORY_INIT,
                path = %path.display(),
                error = %e,
                "failed to read init file",
            ),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn init_reads_agents_md_cursorrules_and_windsurfrules() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("AGENTS.md"), "AGENTS-BODY").unwrap();
        fs::write(root.join(".cursorrules"), "CURSOR-BODY").unwrap();
        fs::write(root.join(".windsurfrules"), "WINDSURF-BODY").unwrap();

        let found = scan_init_files(root);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["AGENTS.md", ".cursorrules", ".windsurfrules"]);
        assert!(found[0].body.contains("AGENTS-BODY"));
        assert!(found[1].body.contains("CURSOR-BODY"));
        assert!(found[2].body.contains("WINDSURF-BODY"));
    }

    #[test]
    fn init_omits_missing_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".cursorrules"), "ONLY-CURSOR").unwrap();
        let found = scan_init_files(root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, ".cursorrules");
    }
}
