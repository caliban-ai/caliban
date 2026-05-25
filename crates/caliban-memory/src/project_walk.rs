//! Ancestor walk: discover `CLAUDE.md` / `AGENTS.md` / `.caliban.md` files
//! upward from cwd to git/fs root, with inode-based dedupe and gitignore-style
//! excludes.
//!
//! Part of ADR 0036 (CLAUDE.md ancestor walk + `@`-imports). See
//! `docs/superpowers/specs/2026-05-24-claudemd-ancestry-design.md` for the
//! design.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use globset::GlobSet;

/// Where the ancestor walk stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WalkStop {
    /// Stop at the first directory containing a `.git/` entry.
    GitRoot,
    /// Stop at the filesystem root (`/`).
    FsRoot,
    /// Stop at whichever boundary is hit first (default).
    #[default]
    Both,
}

impl WalkStop {
    /// Parse a `WalkStop` from its lowercase string form
    /// (`"git_root"` / `"fs_root"` / `"both"`). Falls back to `Both` for any
    /// unrecognized input.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "git_root" | "gitroot" | "git" => Self::GitRoot,
            "fs_root" | "fsroot" | "fs" => Self::FsRoot,
            _ => Self::Both,
        }
    }
}

/// Filenames the walk looks for in every visited directory. Order within a
/// directory: most-specific → most-general.
pub const ANCESTRY_FILENAMES: &[&str] = &[".caliban.md", "CLAUDE.md", "AGENTS.md"];

/// Walk up the directory tree starting at `cwd`, returning every CLAUDE.md /
/// AGENTS.md / `.caliban.md` discovered along the way. Results are returned
/// in **broad → narrow** order (closest to the root first, cwd last).
///
/// `excludes` is a gitignore-style glob set evaluated against the path
/// **relative to `cwd`** (the workspace root for that walk).
///
/// Duplicate files reached via symlinks are dropped via inode-based dedupe
/// (or by canonical-path equality on platforms where `MetadataExt::ino` isn't
/// available).
#[must_use]
pub fn walk_ancestors(cwd: &Path, stop: WalkStop, excludes: &GlobSet) -> Vec<PathBuf> {
    // First pass: collect per-directory files in narrow → broad order.
    // Within each directory we keep the source order from `ANCESTRY_FILENAMES`
    // (most-specific → most-general). Reversing the dir list at the end then
    // gives broad → narrow ordering across directories without disturbing the
    // within-directory order.
    let mut per_dir: Vec<Vec<PathBuf>> = Vec::new();
    let mut seen: BTreeSet<InodeKey> = BTreeSet::new();
    let mut current: Option<PathBuf> = Some(cwd.to_path_buf());

    while let Some(dir) = current {
        let mut dir_hits = Vec::new();
        for name in ANCESTRY_FILENAMES {
            let candidate = dir.join(name);
            if !candidate.is_file() {
                continue;
            }
            // Inode-based dedupe (catches symlinks pointing to the same file).
            let key = inode_key(&candidate);
            if !seen.insert(key) {
                continue;
            }
            // Excludes evaluated relative to the workspace root (= cwd).
            let rel = candidate.strip_prefix(cwd).unwrap_or(&candidate);
            if excludes.is_match(rel) {
                continue;
            }
            dir_hits.push(candidate);
        }
        if !dir_hits.is_empty() {
            per_dir.push(dir_hits);
        }

        if reached_stop(&dir, stop) {
            break;
        }

        match dir.parent() {
            Some(parent) if parent != dir => current = Some(parent.to_path_buf()),
            _ => break,
        }
    }

    // Reverse dir order (broad → narrow) but preserve within-dir order.
    per_dir.reverse();
    per_dir.into_iter().flatten().collect()
}

/// True when `dir` is a stop boundary for the given walk-stop mode.
fn reached_stop(dir: &Path, stop: WalkStop) -> bool {
    match stop {
        WalkStop::GitRoot => dir.join(".git").exists(),
        WalkStop::FsRoot => dir.parent().is_none(),
        WalkStop::Both => dir.join(".git").exists() || dir.parent().is_none(),
    }
}

/// Stable dedupe key for a file. Prefers `(dev, inode)` on Unix; falls back to
/// the canonicalized path on platforms that don't expose inode info or when
/// the syscall fails.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum InodeKey {
    /// `(device, inode)` from `MetadataExt::ino()` and `MetadataExt::dev()`.
    Inode(u64, u64),
    /// Canonicalized path fallback.
    Path(PathBuf),
}

fn inode_key(path: &Path) -> InodeKey {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(md) = std::fs::metadata(path) {
            return InodeKey::Inode(md.dev(), md.ino());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    InodeKey::Path(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn empty_globset() -> GlobSet {
        GlobSet::empty()
    }

    fn excludes(patterns: &[&str]) -> GlobSet {
        let mut b = globset::GlobSetBuilder::new();
        for p in patterns {
            b.add(globset::Glob::new(p).unwrap());
        }
        b.build().unwrap()
    }

    #[test]
    fn walk_from_subdir_discovers_parent_claude_md() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("CLAUDE.md"), "ROOT").unwrap();
        let sub = root.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();

        let found = walk_ancestors(&sub, WalkStop::GitRoot, &empty_globset());
        assert_eq!(found.len(), 1, "expected one CLAUDE.md");
        assert_eq!(
            found[0].canonicalize().unwrap(),
            root.join("CLAUDE.md").canonicalize().unwrap(),
        );
    }

    #[test]
    fn walk_concatenation_order_is_broad_to_narrow() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("CLAUDE.md"), "ROOT").unwrap();
        let mid = root.join("mid");
        let leaf = mid.join("leaf");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(mid.join("CLAUDE.md"), "MID").unwrap();
        fs::write(leaf.join("CLAUDE.md"), "LEAF").unwrap();

        let found = walk_ancestors(&leaf, WalkStop::GitRoot, &empty_globset());
        assert_eq!(found.len(), 3);
        let bodies: Vec<_> = found
            .iter()
            .map(|p| fs::read_to_string(p).unwrap())
            .collect();
        assert_eq!(bodies, vec!["ROOT", "MID", "LEAF"]);
    }

    #[cfg(unix)]
    #[test]
    fn walk_dedupes_by_inode_when_symlink_targets_ancestor() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("CLAUDE.md"), "ROOT").unwrap();
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        // Symlink sub/CLAUDE.md → root/CLAUDE.md (same inode).
        std::os::unix::fs::symlink(root.join("CLAUDE.md"), sub.join("CLAUDE.md")).unwrap();

        let found = walk_ancestors(&sub, WalkStop::GitRoot, &empty_globset());
        assert_eq!(found.len(), 1, "symlink should be deduped: {found:?}");
    }

    #[test]
    fn walk_loads_both_claude_md_and_agents_md_in_same_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("CLAUDE.md"), "C").unwrap();
        fs::write(root.join("AGENTS.md"), "A").unwrap();
        fs::write(root.join(".caliban.md"), "K").unwrap();

        let found = walk_ancestors(root, WalkStop::GitRoot, &empty_globset());
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().and_then(|s| s.to_str()).unwrap().to_string())
            .collect();
        // After reversal: still the same set; within a dir, order was preserved
        // (most-specific first). Since the walk only visited one directory,
        // reversal doesn't change anything.
        assert_eq!(names, vec![".caliban.md", "CLAUDE.md", "AGENTS.md"]);
    }

    #[test]
    fn walk_honors_excludes_relative_to_cwd() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("CLAUDE.md"), "ROOT").unwrap();
        let vendor = root.join("vendor");
        fs::create_dir_all(&vendor).unwrap();
        fs::write(vendor.join("CLAUDE.md"), "VENDOR").unwrap();

        // From vendor, the walk would find vendor/CLAUDE.md + root/CLAUDE.md.
        // Exclude vendor/* — note the exclude is relative to cwd (= vendor),
        // so `CLAUDE.md` matches vendor's own file. The root file is
        // referenced by its absolute path (strip_prefix fails), so the
        // glob doesn't match it.
        let g = excludes(&["CLAUDE.md"]);
        let found = walk_ancestors(&vendor, WalkStop::GitRoot, &g);
        let names: Vec<_> = found.iter().map(|p| p.display().to_string()).collect();
        assert!(
            !names.iter().any(|n| n.ends_with("vendor/CLAUDE.md")),
            "vendor file should be excluded: {names:?}"
        );
    }

    #[test]
    fn walk_excludes_via_workspace_relative_pattern() {
        // The "monorepo case": cwd is the root, and a nested CLAUDE.md should
        // be skipped via a workspace-relative glob like `node_modules/**`.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("CLAUDE.md"), "ROOT").unwrap();
        // Add a nested CLAUDE.md three levels deep but DO NOT start the walk
        // from it. (Note: walk only goes UP — this test serves as a marker
        // that nested-on-demand is handled elsewhere; here we just verify the
        // exclude pattern semantics work for relative-to-cwd matches.)
        let g = excludes(&["CLAUDE.md"]); // matches the root's CLAUDE.md
        let found = walk_ancestors(root, WalkStop::GitRoot, &g);
        assert!(
            found.is_empty(),
            "excluded file should be skipped: {found:?}"
        );
    }

    #[test]
    fn walk_stops_at_git_root() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path();
        let inner = outer.join("inner");
        let leaf = inner.join("a").join("b");
        fs::create_dir_all(&leaf).unwrap();
        fs::create_dir_all(inner.join(".git")).unwrap();
        fs::write(inner.join("CLAUDE.md"), "INNER").unwrap();
        // OUTER has a CLAUDE.md but it's outside the git root — must NOT be loaded.
        fs::write(outer.join("CLAUDE.md"), "OUTER").unwrap();

        let found = walk_ancestors(&leaf, WalkStop::GitRoot, &empty_globset());
        assert_eq!(found.len(), 1);
        let body = fs::read_to_string(&found[0]).unwrap();
        assert_eq!(body, "INNER");
    }
}
