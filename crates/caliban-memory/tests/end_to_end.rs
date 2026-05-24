//! Integration tests for `caliban-memory`.
//!
//! Each test builds a self-contained directory tree under a `TempDir`, calls
//! [`caliban_memory::load`] with a hand-built `MemoryConfig`, and asserts on
//! the resulting [`MemoryPrefix`].

use std::path::PathBuf;

use caliban_memory::{MemoryConfig, load};

fn write(path: &std::path::Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn config_with(global: Option<PathBuf>, project: Option<PathBuf>, auto_dir: PathBuf) -> MemoryConfig {
    MemoryConfig {
        global_path: global,
        project_path: project,
        auto_memory_dir: auto_dir,
        max_tokens: 8_000,
    }
}

#[tokio::test]
async fn end_to_end_with_tempdir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let global_md = tmp.path().join("config/caliban/CLAUDE.md");
    let workspace = tmp.path().join("workspace");
    let project_md = workspace.join("CLAUDE.md");
    let auto_dir = tmp.path().join("data/caliban/projects/ws/memory");
    let memory_md = auto_dir.join("MEMORY.md");

    write(&global_md, "global content here");
    write(&project_md, "project content here");
    write(&memory_md, "# Memory index\n\n- [foo](foo.md) — bar\n");

    let cfg = config_with(
        Some(global_md.clone()),
        Some(project_md.clone()),
        auto_dir.clone(),
    );
    let prefix = load(&cfg).await.unwrap();

    assert!(prefix.global.is_some(), "global should load");
    assert!(prefix.project.is_some(), "project should load");
    assert!(prefix.auto.is_some(), "auto should load");

    // Splice should mention each file's content and the operator's default body
    // in the right order.
    let body = prefix.splice_into("DEFAULT_BODY");
    let g = body.find("global content here").expect("global body");
    let p = body.find("project content here").expect("project body");
    let a = body.find("[foo](foo.md)").expect("auto body");
    let d = body.find("DEFAULT_BODY").expect("default body");
    assert!(g < p && p < a && a < d, "ordering wrong: {body}");

    // Path attributes should appear in the wrapper tags.
    assert!(body.contains(&global_md.display().to_string()));
    assert!(body.contains(&project_md.display().to_string()));
    assert!(body.contains(&memory_md.display().to_string()));
}

#[tokio::test]
async fn end_to_end_seeds_empty_memory_md_on_first_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let auto_dir = tmp.path().join("auto");
    assert!(!auto_dir.exists());

    let cfg = config_with(None, None, auto_dir.clone());
    let prefix = load(&cfg).await.unwrap();

    let memory_md = auto_dir.join("MEMORY.md");
    assert!(memory_md.exists(), "seed MEMORY.md should be created");
    let body = std::fs::read_to_string(&memory_md).unwrap();
    assert!(
        body.contains("# Memory index"),
        "seed should contain '# Memory index'; got: {body}"
    );

    // The in-memory auto tier should also be Some(…) with the seed content +
    // conventions block appended.
    let auto = prefix.auto.as_ref().expect("auto loaded after seed");
    assert!(auto.body.contains("# Memory index"));
    assert!(auto.body.contains("auto-memory conventions follow"));
}

#[tokio::test]
async fn end_to_end_missing_global_and_project_yields_none_tiers() {
    let tmp = tempfile::TempDir::new().unwrap();
    let auto_dir = tmp.path().join("auto");
    // Reference files that don't exist:
    let bogus_global = tmp.path().join("missing-global.md");
    let bogus_project = tmp.path().join("missing-project.md");

    let cfg = config_with(Some(bogus_global), Some(bogus_project), auto_dir);
    let prefix = load(&cfg).await.unwrap();

    assert!(prefix.global.is_none());
    assert!(prefix.project.is_none());
    // Auto tier is always present once seeded.
    assert!(prefix.auto.is_some());
}
