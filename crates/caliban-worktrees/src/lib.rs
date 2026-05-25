//! `caliban-worktrees` — manage isolated git worktrees for sub-agents.
//!
//! See `docs/superpowers/specs/2026-05-24-subagent-worktree-and-fleet-design.md`
//! and ADR 0037.
//!
//! The crate provides a `WorktreeManager` rooted at a repo's
//! `.caliban/worktrees/` directory. Each [`WorktreeSpec`] describes the
//! desired worktree (name, base ref, optional sparse paths, optional
//! symlink directories) and `create()` materializes it; `remove()` cleans
//! it up. The implementation drives **libgit2** via the `git2` crate so
//! errors are structured rather than parsed from `git` stderr.

#![allow(clippy::missing_errors_doc)]

mod config;
mod manager;
mod sparse;
mod symlinks;

pub use config::{BaseRef, WorktreeSpec};
pub use manager::{WorktreeError, WorktreeHandle, WorktreeManager, WorktreeRecord};
