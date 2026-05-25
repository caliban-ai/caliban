//! Walk + `@`-imports + rules + nested-on-demand integration test.
//!
//! Builds a representative project tree, calls `caliban_memory::load`, and
//! asserts on the resulting `ProjectTier` + addendum-loader behavior. Mirrors
//! the integration test outlined in
//! `docs/superpowers/specs/2026-05-24-claudemd-ancestry-design.md`.

use std::fs;
use std::path::Path;

use caliban_memory::project_walk::WalkStop;
use caliban_memory::{AncestryAddendum, MemoryConfig, load};

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

#[tokio::test]
async fn ancestor_walk_and_imports_integration() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join("CLAUDE.md"), "ROOT-PROJECT\n@./shared/conv.md\n");
    write(
        &root.join("shared").join("conv.md"),
        "CONV-BODY\n@../detail.md\n",
    );
    write(&root.join("detail.md"), "DETAIL-BODY\n");
    write(&root.join("sub").join("CLAUDE.md"), "SUB-PROJECT\n");
    let deep_file = root.join("sub").join("deep").join("foo.py");
    write(&deep_file, "print('hi')\n");

    let mut cfg = MemoryConfig::for_test(tmp.path().join("auto"));
    cfg.project_walk_root = root.join("sub").join("deep");
    cfg.project_walk_stop = WalkStop::GitRoot;
    cfg.disable_walk = false;
    cfg.non_interactive = true; // auto-deny external (none exist anyway)

    let prefix = load(&cfg).await.unwrap();
    let project_tier = prefix.project_tier.as_ref().expect("project tier built");

    // base_files = [root/CLAUDE.md, sub/CLAUDE.md] (broad → narrow).
    let names: Vec<String> = project_tier
        .base_files
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    assert_eq!(project_tier.base_files.len(), 2, "names: {names:?}");
    assert!(
        names[0].ends_with("/CLAUDE.md") && !names[0].contains("sub/"),
        "root first: {names:?}",
    );
    assert!(names[1].contains("sub/"), "sub second: {names:?}");

    // Imports tracked separately for /memory provenance.
    let import_paths: Vec<String> = project_tier
        .imports
        .iter()
        .map(|f| f.path.display().to_string())
        .collect();
    assert!(
        import_paths.iter().any(|p| p.ends_with("/shared/conv.md")),
        "expected shared/conv.md import: {import_paths:?}",
    );
    assert!(
        import_paths.iter().any(|p| p.ends_with("/detail.md")),
        "expected nested detail.md import: {import_paths:?}",
    );

    // Splice should contain the imported content inline.
    let body = prefix.splice_into("BODY");
    assert!(body.contains("ROOT-PROJECT"));
    assert!(body.contains("SUB-PROJECT"));
    assert!(body.contains("CONV-BODY"));
    assert!(body.contains("DETAIL-BODY"));

    // Nested-on-demand on `sub/deep/foo.py` should NOT report new files (sub
    // and root CLAUDE.md were already loaded by the initial walk).
    let initial: Vec<_> = project_tier
        .base_files
        .iter()
        .map(|f| f.path.clone())
        .collect();
    let addendum = AncestryAddendum::new(root.to_path_buf(), WalkStop::GitRoot, initial);
    let new_files = addendum.on_path_touched(&deep_file);
    assert!(
        new_files.is_none(),
        "everything already loaded: {new_files:?}",
    );
}

#[tokio::test]
async fn disable_walk_env_falls_back_to_legacy_single_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("CLAUDE.md"), "ROOT-LEGACY\n");
    write(&root.join("sub").join("CLAUDE.md"), "SUB-IGNORED\n");

    let mut cfg = MemoryConfig::for_test(tmp.path().join("auto"));
    cfg.project_path = Some(root.join("CLAUDE.md"));
    cfg.disable_walk = true;
    cfg.project_walk_root = root.to_path_buf();

    let prefix = load(&cfg).await.unwrap();
    let project = prefix.project.expect("legacy project loaded");
    assert!(project.body.contains("ROOT-LEGACY"));
    assert!(
        !project.body.contains("SUB-IGNORED"),
        "walk should be disabled",
    );
    assert!(
        prefix.project_tier.is_none(),
        "disable_walk should skip the rich tier",
    );
}

#[tokio::test]
async fn agents_md_loaded_directly_alongside_claude_md() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join("CLAUDE.md"), "CLAUDE-CONVENTIONS\n");
    write(&root.join("AGENTS.md"), "AGENTS-CONVENTIONS\n");

    let mut cfg = MemoryConfig::for_test(tmp.path().join("auto"));
    cfg.project_walk_root = root.to_path_buf();
    cfg.disable_walk = false;

    let prefix = load(&cfg).await.unwrap();
    let tier = prefix.project_tier.as_ref().unwrap();
    let names: Vec<String> = tier
        .base_files
        .iter()
        .filter_map(|f| {
            f.path
                .file_name()
                .and_then(|s| s.to_str())
                .map(String::from)
        })
        .collect();
    assert!(names.contains(&"CLAUDE.md".to_string()), "{names:?}");
    assert!(names.contains(&"AGENTS.md".to_string()), "{names:?}");
}

#[tokio::test]
async fn claude_md_excludes_skip_matching_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join("CLAUDE.md"), "ROOT\n");
    write(&root.join("vendor").join("CLAUDE.md"), "VENDOR\n");

    let mut cfg = MemoryConfig::for_test(tmp.path().join("auto"));
    // Walk from vendor — its own CLAUDE.md should be excluded.
    cfg.project_walk_root = root.join("vendor");
    cfg.disable_walk = false;
    cfg.claude_md_excludes = caliban_memory::build_excludes(["CLAUDE.md"]).unwrap();

    let prefix = load(&cfg).await.unwrap();
    let tier = prefix.project_tier.unwrap();
    let bodies: Vec<&str> = tier.base_files.iter().map(|f| f.body.as_str()).collect();
    assert!(
        !bodies.iter().any(|b| b.contains("VENDOR")),
        "vendor was excluded: {bodies:?}",
    );
    // Root is referenced by absolute path (strip_prefix fails), so it survives.
    assert!(bodies.iter().any(|b| b.contains("ROOT")), "{bodies:?}");
}

#[tokio::test]
async fn additional_directories_claude_md_includes_extra_walks() {
    let tmp = tempfile::TempDir::new().unwrap();
    let primary = tmp.path().join("primary");
    let extra = tmp.path().join("extra");
    fs::create_dir_all(primary.join(".git")).unwrap();
    fs::create_dir_all(extra.join(".git")).unwrap();
    write(&primary.join("CLAUDE.md"), "PRIMARY\n");
    write(&extra.join("CLAUDE.md"), "EXTRA-FROM-ADD-DIR\n");

    let mut cfg = MemoryConfig::for_test(tmp.path().join("auto"));
    cfg.project_walk_root = primary.clone();
    cfg.disable_walk = false;
    cfg.additional_dirs = vec![extra.clone()];

    // Without the flag, the extra dir contributes nothing.
    cfg.additional_directories_claude_md = false;
    let prefix = load(&cfg).await.unwrap();
    let bodies: Vec<&str> = prefix
        .project_tier
        .as_ref()
        .unwrap()
        .base_files
        .iter()
        .map(|f| f.body.as_str())
        .collect();
    assert!(bodies.iter().any(|b| b.contains("PRIMARY")));
    assert!(!bodies.iter().any(|b| b.contains("EXTRA-FROM-ADD-DIR")));

    // With the flag, the extra dir's CLAUDE.md is appended.
    cfg.additional_directories_claude_md = true;
    let prefix = load(&cfg).await.unwrap();
    let bodies: Vec<&str> = prefix
        .project_tier
        .as_ref()
        .unwrap()
        .base_files
        .iter()
        .map(|f| f.body.as_str())
        .collect();
    assert!(bodies.iter().any(|b| b.contains("PRIMARY")));
    assert!(bodies.iter().any(|b| b.contains("EXTRA-FROM-ADD-DIR")));
}

#[tokio::test]
async fn rules_dot_caliban_directory_loaded_into_active_rules() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join("CLAUDE.md"), "ROOT\n");
    write(
        &root.join(".caliban").join("rules").join("conventions.md"),
        "---\nname: conventions\n---\n\nALWAYS-RULE-BODY\n",
    );
    write(
        &root.join(".caliban").join("rules").join("python.md"),
        "---\nname: python\npaths:\n  - \"**/*.py\"\n---\n\nPYTHON-RULE-BODY\n",
    );

    let mut cfg = MemoryConfig::for_test(tmp.path().join("auto"));
    cfg.project_walk_root = root.to_path_buf();
    cfg.disable_walk = false;

    let prefix = load(&cfg).await.unwrap();
    let tier = prefix.project_tier.as_ref().unwrap();
    let always: Vec<_> = tier
        .active_rules
        .iter()
        .filter(|r| r.body.contains("ALWAYS-RULE-BODY"))
        .collect();
    assert_eq!(always.len(), 1, "always-active rule loaded");
    // path-scoped rule is NOT in active_rules at startup.
    assert!(
        !tier
            .active_rules
            .iter()
            .any(|r| r.body.contains("PYTHON-RULE-BODY")),
        "path-scoped rule should be lazy",
    );
}
