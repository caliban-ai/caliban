//! Nested-on-demand CLAUDE.md loader.
//!
//! After the model `Read`s a file under subdirectory X, any `CLAUDE.md` (or
//! `AGENTS.md` / `.caliban.md`) found in X or its ancestors (between the
//! workspace root and X, exclusive of the root walk's already-loaded files)
//! is added as a system-prompt addendum for the rest of the session.
//!
//! Part of ADR 0036.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::loader::estimate_tokens;
use crate::prefix::TierFile;
use crate::project_walk::{ANCESTRY_FILENAMES, WalkStop};

/// Session-scoped tracker for CLAUDE.md files discovered on-demand by
/// `Read`/`Edit`/`Glob` hooks.
#[derive(Debug)]
pub struct AncestryAddendum {
    workspace_root: PathBuf,
    walk_stop: WalkStop,
    /// Files already accounted for (initial walk + prior addendums).
    loaded: Mutex<BTreeSet<PathBuf>>,
}

impl AncestryAddendum {
    /// Build an addendum tracker primed with `initial_loaded` — the set of
    /// CLAUDE.md files already loaded by the startup walk. Future
    /// `on_path_touched` calls dedupe against this set.
    #[must_use]
    pub fn new(
        workspace_root: PathBuf,
        walk_stop: WalkStop,
        initial_loaded: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        Self {
            workspace_root,
            walk_stop,
            loaded: Mutex::new(initial_loaded.into_iter().collect()),
        }
    }

    /// Notification entrypoint: the model touched `path` (via Read / Edit /
    /// Glob). Returns any **newly discovered** CLAUDE.md / AGENTS.md /
    /// `.caliban.md` files in `path`'s ancestry, between the workspace root
    /// (inclusive) and `path`'s parent (inclusive). Already-loaded files are
    /// elided.
    ///
    /// Returns `None` when nothing new was discovered.
    ///
    /// # Panics
    ///
    /// Panics only if the internal `Mutex` is poisoned (another thread
    /// panicked while holding the lock).
    pub fn on_path_touched(&self, path: &Path) -> Option<Vec<TierFile>> {
        let mut new_files = Vec::new();
        let mut current = path.parent().map(Path::to_path_buf);

        let mut loaded = self.loaded.lock().expect("addendum mutex poisoned");
        while let Some(dir) = current.clone() {
            for name in ANCESTRY_FILENAMES {
                let candidate = dir.join(name);
                if !candidate.is_file() {
                    continue;
                }
                if !loaded.insert(candidate.clone()) {
                    continue;
                }
                let body = match std::fs::read(&candidate) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => {
                        tracing::warn!(
                            target: "caliban::memory",
                            path = %candidate.display(),
                            error = %e,
                            "failed to read nested CLAUDE.md",
                        );
                        continue;
                    }
                };
                let estimated_tokens = estimate_tokens(&body);
                new_files.push(TierFile {
                    path: candidate,
                    body,
                    estimated_tokens,
                    truncated_bytes: 0,
                });
            }
            // Stop when we've crossed the workspace root.
            if dir == self.workspace_root {
                break;
            }
            // Stop also when the walk_stop boundary says so (e.g. .git/).
            if matches!(self.walk_stop, WalkStop::GitRoot | WalkStop::Both)
                && dir.join(".git").exists()
            {
                break;
            }
            current = dir.parent().map(Path::to_path_buf);
        }

        if new_files.is_empty() {
            None
        } else {
            Some(new_files)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn nested_on_demand_returns_subtree_claude_md() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        let sub = root.join("backend");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CLAUDE.md"), "BACKEND-CONVENTIONS").unwrap();

        let addendum =
            AncestryAddendum::new(root.to_path_buf(), WalkStop::GitRoot, std::iter::empty());
        let touched = sub.join("server.rs");
        fs::write(&touched, "fn main() {}").unwrap();
        let new_files = addendum.on_path_touched(&touched).expect("loaded");
        assert_eq!(new_files.len(), 1);
        assert!(new_files[0].body.contains("BACKEND-CONVENTIONS"));
    }

    #[test]
    fn nested_on_demand_dedupes_after_first_touch() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        let sub = root.join("backend");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CLAUDE.md"), "X").unwrap();

        let addendum =
            AncestryAddendum::new(root.to_path_buf(), WalkStop::GitRoot, std::iter::empty());
        let f1 = sub.join("a.rs");
        let f2 = sub.join("b.rs");
        fs::write(&f1, "").unwrap();
        fs::write(&f2, "").unwrap();
        let first = addendum.on_path_touched(&f1);
        let second = addendum.on_path_touched(&f2);
        assert!(first.is_some());
        assert!(
            second.is_none(),
            "second touch should not re-load: {second:?}"
        );
    }

    #[test]
    fn nested_on_demand_skips_files_initial_walk_already_loaded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        let sub = root.join("backend");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CLAUDE.md"), "BACKEND").unwrap();

        // Pretend the initial walk already saw sub/CLAUDE.md.
        let addendum = AncestryAddendum::new(
            root.to_path_buf(),
            WalkStop::GitRoot,
            std::iter::once(sub.join("CLAUDE.md")),
        );
        let touched = sub.join("server.rs");
        fs::write(&touched, "").unwrap();
        let new_files = addendum.on_path_touched(&touched);
        assert!(new_files.is_none(), "already-loaded file should be skipped");
    }

    #[test]
    fn nested_on_demand_stops_at_workspace_root() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path();
        let workspace = outer.join("ws");
        let sub = workspace.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        // CLAUDE.md OUTSIDE the workspace must not be picked up.
        fs::write(outer.join("CLAUDE.md"), "OUTSIDE").unwrap();
        fs::write(workspace.join("CLAUDE.md"), "WS").unwrap();
        fs::write(sub.join("CLAUDE.md"), "SUB").unwrap();

        let addendum =
            AncestryAddendum::new(workspace.clone(), WalkStop::FsRoot, std::iter::empty());
        let touched = sub.join("file.rs");
        fs::write(&touched, "").unwrap();
        let new_files = addendum.on_path_touched(&touched).expect("loaded");
        let bodies: Vec<_> = new_files.iter().map(|f| f.body.as_str()).collect();
        assert!(
            bodies.contains(&"WS"),
            "workspace CLAUDE.md loaded: {bodies:?}"
        );
        assert!(bodies.contains(&"SUB"));
        assert!(
            !bodies.iter().any(|b| b == &"OUTSIDE"),
            "must not cross workspace root: {bodies:?}",
        );
    }
}
