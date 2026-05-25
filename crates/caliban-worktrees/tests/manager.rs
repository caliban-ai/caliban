//! Integration tests for [`caliban_worktrees::WorktreeManager`].

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use caliban_worktrees::{BaseRef, WorktreeManager, WorktreeSpec};

/// Create a temporary git repo with one committed file. Returns the
/// `TempDir` (kept for lifetime) and the path inside it.
fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    let repo = git2::Repository::init(&path).expect("init repo");

    // Configure identity so commit() works.
    let mut cfg = repo.config().expect("config");
    cfg.set_str("user.name", "caliban-test").unwrap();
    cfg.set_str("user.email", "test@caliban.invalid").unwrap();

    // Write a seed file and commit it.
    let seed = path.join("README.md");
    fs::write(&seed, "seed\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = repo.signature().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])
        .unwrap();
    (dir, path)
}

fn git_binary_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn new_rejects_non_repo() {
    let dir = tempfile::tempdir().unwrap();
    let err = WorktreeManager::new(dir.path()).unwrap_err();
    assert!(matches!(err, caliban_worktrees::WorktreeError::NotARepo(_)));
}

#[test]
fn new_opens_real_repo() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    assert_eq!(mgr.repo_root(), repo.as_path());
    assert_eq!(
        mgr.worktrees_root(),
        repo.join(".caliban").join("worktrees")
    );
}

#[test]
fn create_with_base_ref_head_materializes_files() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let spec = WorktreeSpec::new("wt-head").base_ref(BaseRef::Head);
    let handle = mgr.create(&spec).unwrap();
    assert!(handle.path.is_dir(), "worktree dir should exist");
    assert!(
        handle.path.join("README.md").exists(),
        "seed file from HEAD should be in worktree"
    );
    assert_eq!(handle.branch, "caliban/wt-head");
}

#[test]
fn create_with_base_ref_fresh_writes_marker_pattern() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let spec = WorktreeSpec::new("wt-fresh").base_ref(BaseRef::Fresh);
    let _h = mgr.create(&spec).unwrap();
    // Sparse-checkout file lives in <repo>/.git/worktrees/wt-fresh/info/sparse-checkout
    let sparse = repo
        .join(".git")
        .join("worktrees")
        .join("wt-fresh")
        .join("info")
        .join("sparse-checkout");
    assert!(sparse.exists(), "fresh base_ref should write a sparse file");
    let body = fs::read_to_string(&sparse).unwrap();
    assert!(body.contains(".caliban-fresh-empty-marker"));
}

#[test]
fn sparse_paths_writes_sparse_checkout_file() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let spec = WorktreeSpec::new("wt-sparse")
        .base_ref(BaseRef::Head)
        .sparse_paths(vec!["crates/foo/".into(), "docs/".into()]);
    let _h = mgr.create(&spec).unwrap();
    let sparse = repo
        .join(".git")
        .join("worktrees")
        .join("wt-sparse")
        .join("info")
        .join("sparse-checkout");
    let body = fs::read_to_string(&sparse).unwrap();
    assert!(body.contains("crates/foo/"));
    assert!(body.contains("docs/"));
}

#[test]
fn symlink_directories_links_into_parent() {
    let (_keep, repo) = init_repo();
    fs::create_dir_all(repo.join("target")).unwrap();
    fs::write(repo.join("target").join("dummy.txt"), "x").unwrap();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let spec = WorktreeSpec::new("wt-syms")
        .base_ref(BaseRef::Head)
        .symlink_directories(vec![PathBuf::from("target")]);
    let handle = mgr.create(&spec).unwrap();
    let linked = handle.path.join("target");
    let meta = fs::symlink_metadata(&linked).unwrap();
    assert!(meta.file_type().is_symlink(), "target/ should be a symlink");
    // And the link should resolve to the parent's target/.
    assert!(linked.join("dummy.txt").exists());
}

#[test]
fn symlink_directories_rejects_missing_parent_path() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let spec = WorktreeSpec::new("wt-syms-bad")
        .base_ref(BaseRef::Head)
        .symlink_directories(vec![PathBuf::from("does-not-exist")]);
    let err = mgr.create(&spec).unwrap_err();
    assert!(matches!(
        err,
        caliban_worktrees::WorktreeError::BrokenLink { .. }
    ));
}

#[test]
fn create_with_named_ref_resolves_to_commit() {
    let (_keep, repo) = init_repo();
    // Capture HEAD sha and pass it explicitly as the base_ref.
    let r = git2::Repository::open(&repo).unwrap();
    let head_oid = r.head().unwrap().peel_to_commit().unwrap().id().to_string();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let spec = WorktreeSpec::new("wt-named").base_ref(BaseRef::Ref(head_oid));
    let handle = mgr.create(&spec).unwrap();
    assert!(handle.path.join("README.md").exists());
}

#[test]
fn create_refuses_when_name_already_exists() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let spec = WorktreeSpec::new("dup");
    let _h = mgr.create(&spec).unwrap();
    let err = mgr.create(&spec).unwrap_err();
    assert!(matches!(
        err,
        caliban_worktrees::WorktreeError::AlreadyExists(_)
    ));
}

#[test]
fn list_returns_managed_worktrees() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    assert!(mgr.list().unwrap().is_empty());
    let _a = mgr.create(&WorktreeSpec::new("alpha")).unwrap();
    let _b = mgr.create(&WorktreeSpec::new("beta")).unwrap();
    let mut names: Vec<_> = mgr.list().unwrap().into_iter().map(|r| r.name).collect();
    names.sort();
    assert_eq!(names, vec!["alpha", "beta"]);
}

#[test]
fn remove_drops_directory_and_branch() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let handle = mgr.create(&WorktreeSpec::new("wt-rm")).unwrap();
    assert!(handle.path.exists());
    mgr.remove("wt-rm", true).unwrap();
    assert!(!handle.path.exists(), "directory should be gone");
    // Branch deleted too.
    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        r.find_branch("caliban/wt-rm", git2::BranchType::Local)
            .is_err()
    );
}

#[test]
fn remove_unknown_name_errors() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let err = mgr.remove("ghost", false).unwrap_err();
    assert!(matches!(err, caliban_worktrees::WorktreeError::NotFound(_)));
}

#[test]
fn worktree_dir_layout_matches_spec() {
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let h = mgr.create(&WorktreeSpec::new("layout")).unwrap();
    let expected = repo.join(".caliban").join("worktrees").join("layout");
    assert_eq!(h.path, expected);
}

#[test]
fn git_binary_sees_worktree_when_available() {
    if !git_binary_available() {
        eprintln!("git binary unavailable; skipping smoke test");
        return;
    }
    let (_keep, repo) = init_repo();
    let mgr = WorktreeManager::new(&repo).unwrap();
    let _h = mgr.create(&WorktreeSpec::new("smoke")).unwrap();
    let out = Command::new("git")
        .arg("worktree")
        .arg("list")
        .current_dir(&repo)
        .output()
        .expect("git worktree list");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("smoke"), "git worktree list output:\n{s}");
}
