//! Capture git build provenance for `caliban --version` (#303).
//!
//! The crate semver alone can't tell two builds from different commits on the
//! same `main` apart. Here we shell out to git at build time and expose the
//! short SHA, a dirty-worktree marker, and the commit date to the crate via
//! `cargo:rustc-env`, where `src/version.rs` folds them into the `--version`
//! string. Every git interaction degrades to an empty string when git or the
//! `.git` directory is absent (crates.io publishes, source tarballs, sandboxed
//! builds), so a metadata-less build cleanly reports just the semver and the
//! build never fails on account of missing provenance.

use std::process::Command;

fn main() {
    // Always define the vars so `option_env!` in the crate resolves to
    // `Some("")` rather than `None` on a no-git build — both are treated as
    // "no metadata", but emitting them keeps the two paths identical.
    let sha = git(&["rev-parse", "--short", "HEAD"]);
    let dirty = if !sha.is_empty() && worktree_dirty() {
        "-dirty"
    } else {
        ""
    };
    // Commit date (UTC, YYYY-MM-DD) ties the date to the SHA rather than to
    // wall-clock build time, keeping the string reproducible for a given commit.
    let date = git(&[
        "show",
        "-s",
        "--format=%cd",
        "--date=format-local:%Y-%m-%d",
        "HEAD",
    ]);

    println!("cargo:rustc-env=CALIBAN_GIT_SHA={sha}");
    println!("cargo:rustc-env=CALIBAN_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=CALIBAN_COMMIT_DATE={date}");

    emit_rerun_triggers();
}

/// Run `git <args>` from the crate dir, returning trimmed stdout or `""` on any
/// failure (git missing, not a repo, non-zero exit).
fn git(args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// True when the working tree has staged or unstaged changes.
fn worktree_dirty() -> bool {
    // `--porcelain` prints one line per changed path; empty output = clean.
    !git(&["status", "--porcelain"]).is_empty()
}

/// Rebuild when the checked-out commit changes so the embedded SHA never goes
/// stale between commits. Resolves the real git dir (correct even inside a
/// linked worktree) and watches HEAD plus the branch ref it points at.
fn emit_rerun_triggers() {
    let git_dir = git(&["rev-parse", "--absolute-git-dir"]);
    if git_dir.is_empty() {
        return;
    }
    println!("cargo:rerun-if-changed={git_dir}/HEAD");
    // On a branch, the SHA lives in the ref file; watch it too. Detached HEAD
    // has no symbolic ref, so this is skipped and HEAD alone suffices.
    let head_ref = git(&["symbolic-ref", "-q", "HEAD"]);
    if !head_ref.is_empty() {
        // The ref may be packed rather than a loose file; watching the loose
        // path is harmless if absent and catches the common (loose) case.
        println!("cargo:rerun-if-changed={git_dir}/{head_ref}");
    }
}
