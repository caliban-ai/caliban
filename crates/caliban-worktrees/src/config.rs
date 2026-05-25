//! Worktree spec types — what to create, off of what base.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What ref a new worktree should be rooted at.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BaseRef {
    /// Branch off an empty tree (`4b825dc...`) so the worktree starts
    /// empty. Operator scripts materialize files deliberately.
    Fresh,
    /// Branch off the current HEAD of the parent repo. Default.
    #[default]
    Head,
    /// Any rev-parse-able ref string (branch name, tag, sha).
    Ref(String),
}

/// Declarative description of a worktree the manager should create.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeSpec {
    /// Becomes the directory name under `.caliban/worktrees/<name>/` and
    /// the name of the underlying git branch (prefixed with `caliban/`).
    pub name: String,
    /// What to root the new worktree on.
    #[serde(default)]
    pub base_ref: BaseRef,
    /// Optional sparse-checkout patterns written into
    /// `.git/info/sparse-checkout` for the worktree.
    #[serde(default)]
    pub sparse_paths: Vec<String>,
    /// Paths under the parent repo to symlink into the worktree (e.g.
    /// `target/`, `node_modules/`). Must exist in the parent repo at
    /// creation time.
    #[serde(default)]
    pub symlink_directories: Vec<PathBuf>,
}

impl WorktreeSpec {
    /// Build a minimal spec with the given name and default settings
    /// (branch off HEAD, no sparse, no symlinks).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_ref: BaseRef::default(),
            sparse_paths: Vec::new(),
            symlink_directories: Vec::new(),
        }
    }

    /// Builder: set the base ref.
    #[must_use]
    pub fn base_ref(mut self, b: BaseRef) -> Self {
        self.base_ref = b;
        self
    }

    /// Builder: set the sparse patterns.
    #[must_use]
    pub fn sparse_paths(mut self, paths: Vec<String>) -> Self {
        self.sparse_paths = paths;
        self
    }

    /// Builder: set the symlink directories.
    #[must_use]
    pub fn symlink_directories(mut self, paths: Vec<PathBuf>) -> Self {
        self.symlink_directories = paths;
        self
    }
}
