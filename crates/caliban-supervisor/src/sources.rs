//! Workspace source discovery + resolution (#281 / ADR 0052).
//!
//! A *source* is a git checkout under the workspace root. caliband roots a
//! workspace directory holding N sources; an agent's `SpawnSpec.source` names
//! which one it runs against.

use std::path::{Path, PathBuf};

/// A discovered source checkout within a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Directory name (the identifier used by `SpawnSpec.source`).
    pub name: String,
    /// Absolute path to the checkout.
    pub path: PathBuf,
}

fn is_checkout(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Enumerate git checkouts under `workspace_root` (immediate children with a
/// `.git`), plus the root itself when it is a checkout. Sorted by name.
pub fn discover_sources(workspace_root: &Path) -> Vec<Source> {
    let mut out = Vec::new();
    if is_checkout(workspace_root)
        && let Some(name) = workspace_root.file_name().and_then(|n| n.to_str())
    {
        out.push(Source {
            name: name.to_string(),
            path: workspace_root.to_path_buf(),
        });
    }
    if let Ok(entries) = std::fs::read_dir(workspace_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && is_checkout(&path)
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                out.push(Source {
                    name: name.to_string(),
                    path,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Resolve `source` (a source name) to an absolute directory under
/// `workspace_root`. `None` resolves to the workspace root itself.
pub fn resolve_source(workspace_root: &Path, source: Option<&str>) -> std::io::Result<PathBuf> {
    let Some(name) = source else {
        return Ok(workspace_root.to_path_buf());
    };
    // Guard against traversal: a source name is a single path component.
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid source name: {name}"),
        ));
    }
    if let Some(src) = discover_sources(workspace_root)
        .into_iter()
        .find(|s| s.name == name)
    {
        return Ok(src.path);
    }
    let candidate = workspace_root.join(name);
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no such source: {name}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_checkout(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn discovers_child_checkouts_and_resolves() {
        let ws = tempfile::tempdir().unwrap();
        git_checkout(&ws.path().join("caliban"));
        git_checkout(&ws.path().join("gonzalo"));
        std::fs::create_dir_all(ws.path().join("not-a-repo")).unwrap();

        let mut names: Vec<_> = discover_sources(ws.path())
            .into_iter()
            .map(|s| s.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["caliban", "gonzalo"]);

        assert_eq!(
            resolve_source(ws.path(), Some("gonzalo")).unwrap(),
            ws.path().join("gonzalo")
        );
        assert_eq!(resolve_source(ws.path(), None).unwrap(), ws.path());
        assert_eq!(
            resolve_source(ws.path(), Some("missing"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(
            resolve_source(ws.path(), Some("../escape"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn root_itself_is_a_source_when_a_checkout() {
        let ws = tempfile::tempdir().unwrap();
        git_checkout(ws.path());
        let names: Vec<_> = discover_sources(ws.path())
            .into_iter()
            .map(|s| s.name)
            .collect();
        // The workspace root's own dir name is a source (single-source back-compat).
        assert_eq!(names.len(), 1);
    }
}
