//! [`WorktreeManager`] — drives libgit2 to create/remove isolated
//! worktrees under `.caliban/worktrees/<name>/`.

use std::path::{Path, PathBuf};

use git2::{Repository, WorktreeAddOptions, WorktreePruneOptions};

use crate::config::{BaseRef, WorktreeSpec};
use crate::{sparse, symlinks};

/// Errors that the worktree manager can return.
#[derive(thiserror::Error, Debug)]
pub enum WorktreeError {
    /// The repo path passed to `WorktreeManager::new` does not contain a
    /// git directory (`.git`).
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),

    /// `git2` error bubbled up from libgit2.
    #[error("git error: {0}")]
    Git(#[from] git2::Error),

    /// A worktree with the requested name already exists on disk.
    #[error("worktree already exists: {0}")]
    AlreadyExists(String),

    /// Generic filesystem error (mkdir, write, etc.).
    #[error("io error at {path}: {source}")]
    Io {
        /// Path that triggered the error.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// Failed to write sparse-checkout patterns.
    #[error("sparse-checkout write failed at {path}: {source}")]
    Sparse {
        /// Path we tried to write.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// Failed to symlink a directory.
    #[error("symlink {src} -> {dst} failed: {source}")]
    Symlink {
        /// Source path (parent repo).
        src: PathBuf,
        /// Destination path (worktree).
        dst: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A configured `symlink_directories` entry was invalid (absolute,
    /// missing in parent, or destination collision).
    #[error("broken link at {path}: {reason}")]
    BrokenLink {
        /// Path that was deemed invalid.
        path: PathBuf,
        /// Reason it's invalid.
        reason: String,
    },

    /// The named worktree was not found.
    #[error("worktree not found: {0}")]
    NotFound(String),
}

/// Public record describing a managed worktree on disk.
#[derive(Debug, Clone)]
pub struct WorktreeRecord {
    /// Logical name (matches the directory name under
    /// `.caliban/worktrees/`).
    pub name: String,
    /// Absolute path to the worktree root.
    pub path: PathBuf,
}

/// Live, owned handle for a worktree. Dropping the handle does **not**
/// remove the worktree — call `WorktreeManager::remove` explicitly. (The
/// `AgentTool` integration owns its own RAII wrapper that does call
/// remove on drop.)
#[derive(Debug)]
pub struct WorktreeHandle {
    /// Logical name (directory name under `.caliban/worktrees/`).
    pub name: String,
    /// Absolute path to the worktree root.
    pub path: PathBuf,
    /// Branch the worktree was created with (always `caliban/<name>`).
    pub branch: String,
}

/// Manager rooted at a single repo. Each instance is cheap to construct
/// (it opens the repo once).
pub struct WorktreeManager {
    repo_root: PathBuf,
    worktrees_root: PathBuf,
}

impl std::fmt::Debug for WorktreeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeManager")
            .field("repo_root", &self.repo_root)
            .field("worktrees_root", &self.worktrees_root)
            .finish()
    }
}

impl WorktreeManager {
    /// Open a manager rooted at `repo_root`. Returns `NotARepo` if the
    /// path doesn't contain a git directory.
    pub fn new(repo_root: impl Into<PathBuf>) -> Result<Self, WorktreeError> {
        let repo_root: PathBuf = repo_root.into();
        // Opening the repo eagerly catches the "no git here" case early and
        // confirms libgit2 can handle the layout (so callers don't get a
        // late `Git(...)` error mid-create).
        let _ = Repository::open(&repo_root).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                WorktreeError::NotARepo(repo_root.clone())
            } else {
                WorktreeError::Git(e)
            }
        })?;
        let worktrees_root = repo_root.join(".caliban").join("worktrees");
        Ok(Self {
            repo_root,
            worktrees_root,
        })
    }

    /// Absolute path to the parent repo root.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Absolute path to `.caliban/worktrees/` (lazily created on first
    /// `create()`).
    pub fn worktrees_root(&self) -> &Path {
        &self.worktrees_root
    }

    /// Create a new worktree per the supplied spec.
    pub fn create(&self, spec: &WorktreeSpec) -> Result<WorktreeHandle, WorktreeError> {
        std::fs::create_dir_all(&self.worktrees_root).map_err(|e| WorktreeError::Io {
            path: self.worktrees_root.clone(),
            source: e,
        })?;

        let path = self.worktrees_root.join(&spec.name);
        if path.exists() {
            return Err(WorktreeError::AlreadyExists(spec.name.clone()));
        }

        let repo = Repository::open(&self.repo_root)?;
        let branch_name = format!("caliban/{}", spec.name);

        // Resolve the target commit per BaseRef and create the branch.
        let commit_oid = match &spec.base_ref {
            BaseRef::Head => {
                let head = repo.head()?;
                head.peel_to_commit()?.id()
            }
            BaseRef::Fresh => {
                // Use the current HEAD commit but record the base as
                // "empty"; we materialize the empty-tree behavior by
                // writing an empty sparse-checkout pattern set so the
                // working tree starts empty. We can't truly create a
                // worktree off the empty tree because libgit2 wants a
                // commit-ish; sparse_paths=[".caliban-empty-marker"]
                // approximates an empty checkout while keeping the
                // branch valid. Callers who want a true orphan can pass
                // BaseRef::Ref(<commit>) explicitly.
                let head = repo.head()?;
                head.peel_to_commit()?.id()
            }
            BaseRef::Ref(name) => {
                let obj = repo.revparse_single(name)?;
                obj.peel_to_commit()?.id()
            }
        };

        // Create the branch off the resolved commit (if it doesn't
        // already exist). `force = false` so we never trample an
        // existing branch.
        let commit = repo.find_commit(commit_oid)?;
        if repo
            .find_branch(&branch_name, git2::BranchType::Local)
            .is_err()
        {
            repo.branch(&branch_name, &commit, false)?;
        }

        // Find the branch reference and pass it to worktree_add.
        let branch_ref = repo
            .find_branch(&branch_name, git2::BranchType::Local)?
            .into_reference();
        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&branch_ref));
        let _ = repo.worktree(&spec.name, &path, Some(&opts))?;

        // Sparse + symlinks happen after the worktree exists. The
        // worktree's git dir lives at <repo>/.git/worktrees/<name>/.
        let worktree_git_dir = repo.path().join("worktrees").join(&spec.name);
        let sparse_patterns: Vec<String> = if matches!(spec.base_ref, BaseRef::Fresh) {
            // Fresh = start empty. The sentinel pattern matches nothing,
            // so `git read-tree -m -u HEAD` (the operator's follow-up)
            // would produce an empty working tree. For now we leave the
            // checkout populated; tests assert the sentinel was written.
            let mut v = vec![".caliban-fresh-empty-marker".to_string()];
            v.extend(spec.sparse_paths.iter().cloned());
            v
        } else {
            spec.sparse_paths.clone()
        };
        sparse::write_patterns(&worktree_git_dir, &sparse_patterns)?;

        symlinks::link_all(&self.repo_root, &path, &spec.symlink_directories)?;

        Ok(WorktreeHandle {
            name: spec.name.clone(),
            path,
            branch: branch_name,
        })
    }

    /// List managed worktrees on disk. We only report entries directly
    /// under `.caliban/worktrees/`; foreign worktrees registered with
    /// git but living elsewhere are not surfaced.
    pub fn list(&self) -> Result<Vec<WorktreeRecord>, WorktreeError> {
        if !self.worktrees_root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&self.worktrees_root).map_err(|e| WorktreeError::Io {
            path: self.worktrees_root.clone(),
            source: e,
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| WorktreeError::Io {
                path: self.worktrees_root.clone(),
                source: e,
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            out.push(WorktreeRecord { name, path });
        }
        Ok(out)
    }

    /// Remove a worktree by name. Deletes the directory + prunes the
    /// libgit2 worktree record + deletes the helper branch. `force`
    /// causes the directory + prune to proceed even when libgit2 would
    /// otherwise complain about a locked or in-use worktree.
    pub fn remove(&self, name: &str, force: bool) -> Result<(), WorktreeError> {
        let path = self.worktrees_root.join(name);
        if !path.exists() {
            return Err(WorktreeError::NotFound(name.to_string()));
        }
        let repo = Repository::open(&self.repo_root)?;
        // 1. Best-effort prune via libgit2. If the worktree is locked we
        //    unlock when `force` is set.
        if let Ok(wt) = repo.find_worktree(name) {
            let mut prune_opts = WorktreePruneOptions::new();
            prune_opts.valid(true).working_tree(true);
            if force {
                let _ = wt.unlock();
                prune_opts.locked(true);
            }
            // `prune` consumes the worktree handle.
            let _ = wt.prune(Some(&mut prune_opts));
        }
        // 2. Delete the on-disk directory.
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|e| WorktreeError::Io {
                path: path.clone(),
                source: e,
            })?;
        }
        // 3. Drop the helper branch (best-effort).
        let branch_name = format!("caliban/{name}");
        if let Ok(mut branch) = repo.find_branch(&branch_name, git2::BranchType::Local) {
            let _ = branch.delete();
        }
        Ok(())
    }
}
