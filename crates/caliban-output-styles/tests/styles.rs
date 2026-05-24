//! Integration tests for output-style loading, selection, and splicing.

#![allow(clippy::multiple_crate_versions)]

use std::fs;
use std::path::PathBuf;

use caliban_agent_core::AssistantPostProcessor;
use caliban_output_styles::{
    DiscoveryRoots, LearningPostProcessor, OutputStylePrefix, OutputStylesRegistry, load_one,
    load_styles, select_active, style::OutputStyleSource,
};
use tempfile::TempDir;

/// Helper: build a `DiscoveryRoots` that points each tier at one tempdir.
/// `plugin_root_dir`, if `Some`, is used as the parent of `<plugin>/output-styles/`.
fn roots_in(
    project: PathBuf,
    user: Option<PathBuf>,
    plugins_root: Option<PathBuf>,
) -> DiscoveryRoots {
    DiscoveryRoots {
        project,
        user,
        plugins_root,
    }
}

fn write_style(dir: &PathBuf, name: &str, body: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("mkdir");
    let path = dir.join(format!("{name}.md"));
    fs::write(&path, body).expect("write");
    path
}

const TIGHTLIPPED: &str = "---\n\
name: tightlipped\n\
description: \"Minimal responses; essentials only.\"\n\
keep_coding_instructions: true\n\
force_for_plugin: false\n\
---\n\
\n\
Respond with minimum prose.\n";

const NO_CODING: &str = "---\n\
name: noco\n\
description: \"Documentation-only mode.\"\n\
keep_coding_instructions: false\n\
---\n\
\n\
You produce documentation only.\n";

const PLUGIN_FORCING: &str = "---\n\
name: pinned\n\
description: \"Locked by plugin.\"\n\
keep_coding_instructions: true\n\
force_for_plugin: true\n\
---\n\
\n\
You are pinned.\n";

const KEBAB_CASE: &str = "---\n\
name: kebabby\n\
description: \"Uses kebab-case aliases.\"\n\
keep-coding-instructions: false\n\
force-for-plugin: false\n\
---\n\
\n\
Body.\n";

// ---------------------------------------------------------------------------
// Frontmatter parsing
// ---------------------------------------------------------------------------

#[test]
fn frontmatter_parses_all_four_fields() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = write_style(&dir, "tightlipped", TIGHTLIPPED);
    let style = load_one(&path, OutputStyleSource::User { path: path.clone() }).expect("loads");
    assert_eq!(style.name, "tightlipped");
    assert_eq!(style.description, "Minimal responses; essentials only.");
    assert!(style.keep_coding_instructions);
    assert!(!style.force_for_plugin);
    assert!(style.body.contains("Respond with minimum prose."));
}

#[test]
fn frontmatter_keep_coding_instructions_false_parses() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = write_style(&dir, "noco", NO_CODING);
    let style = load_one(&path, OutputStyleSource::User { path: path.clone() }).expect("loads");
    assert!(!style.keep_coding_instructions);
}

#[test]
fn frontmatter_accepts_kebab_case_aliases() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = write_style(&dir, "kebabby", KEBAB_CASE);
    let style = load_one(&path, OutputStyleSource::User { path: path.clone() }).expect("loads");
    assert!(!style.keep_coding_instructions);
    assert!(!style.force_for_plugin);
}

#[test]
fn frontmatter_missing_delimiters_rejected() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bare.md");
    fs::write(&path, "no frontmatter here\n").unwrap();
    let err = load_one(&path, OutputStyleSource::User { path: path.clone() }).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("frontmatter"), "{msg}");
}

#[test]
fn frontmatter_name_must_match_filename_stem() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    // Body says `name: tightlipped` but the file is `wrongname.md`.
    let path = write_style(&dir, "wrongname", TIGHTLIPPED);
    let err = load_one(&path, OutputStyleSource::User { path: path.clone() }).unwrap_err();
    assert!(format!("{err}").contains("does not match"), "{err}");
}

// ---------------------------------------------------------------------------
// Built-ins
// ---------------------------------------------------------------------------

#[test]
fn all_four_builtins_load() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing-project"), None, None);
    let styles = load_styles(&roots);
    let names: Vec<&str> = styles.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"default"));
    assert!(names.contains(&"proactive"));
    assert!(names.contains(&"explanatory"));
    assert!(names.contains(&"learning"));
    assert_eq!(
        styles.len(),
        4,
        "should be exactly the four built-ins, got: {names:?}",
    );
}

#[test]
fn proactive_explanatory_learning_have_non_empty_bodies() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing-project"), None, None);
    let styles = load_styles(&roots);
    for name in ["proactive", "explanatory", "learning"] {
        let s = styles.iter().find(|s| s.name == name).expect(name);
        assert!(
            !s.body.trim().is_empty(),
            "{name} body must be non-empty for the splice to do anything",
        );
        assert!(
            matches!(s.source, OutputStyleSource::BuiltIn),
            "{name} must have BuiltIn source",
        );
    }
}

#[test]
fn default_builtin_has_empty_body() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing-project"), None, None);
    let styles = load_styles(&roots);
    let d = styles
        .iter()
        .find(|s| s.name == "default")
        .expect("default present");
    assert!(d.body.trim().is_empty(), "default body must be empty");
}

// ---------------------------------------------------------------------------
// Discovery + shadowing
// ---------------------------------------------------------------------------

#[test]
fn project_root_scanned() {
    let tmp = TempDir::new().unwrap();
    let project = tmp
        .path()
        .join("proj")
        .join(".caliban")
        .join("output-styles");
    write_style(&project, "tightlipped", TIGHTLIPPED);
    let roots = roots_in(project, None, None);
    let styles = load_styles(&roots);
    assert!(
        styles.iter().any(|s| s.name == "tightlipped"),
        "project style missing"
    );
}

#[test]
fn user_root_scanned() {
    let tmp = TempDir::new().unwrap();
    let project = tmp
        .path()
        .join("proj")
        .join(".caliban")
        .join("output-styles");
    let user = tmp
        .path()
        .join("user")
        .join("caliban")
        .join("output-styles");
    write_style(&user, "tightlipped", TIGHTLIPPED);
    let roots = roots_in(project, Some(user), None);
    let styles = load_styles(&roots);
    let t = styles
        .iter()
        .find(|s| s.name == "tightlipped")
        .expect("user style missing");
    assert!(matches!(t.source, OutputStyleSource::User { .. }));
}

#[test]
fn project_wins_on_name_collision_with_user() {
    let tmp = TempDir::new().unwrap();
    let project = tmp
        .path()
        .join("proj")
        .join(".caliban")
        .join("output-styles");
    let user = tmp
        .path()
        .join("user")
        .join("caliban")
        .join("output-styles");
    // Write the same name to both roots; project body differs.
    write_style(&project, "tightlipped", TIGHTLIPPED);
    // user-tier copy with a distinguishable description
    let user_body = TIGHTLIPPED.replace(
        "Minimal responses; essentials only.",
        "USER TIER COPY description",
    );
    write_style(&user, "tightlipped", &user_body);
    let roots = roots_in(project, Some(user), None);
    let styles = load_styles(&roots);
    let t = styles
        .iter()
        .find(|s| s.name == "tightlipped")
        .expect("present");
    assert!(matches!(t.source, OutputStyleSource::Project { .. }));
    assert_eq!(t.description, "Minimal responses; essentials only.");
}

#[test]
fn user_style_shadows_builtin_with_same_name() {
    let tmp = TempDir::new().unwrap();
    let project = tmp
        .path()
        .join("proj")
        .join(".caliban")
        .join("output-styles");
    let user = tmp
        .path()
        .join("user")
        .join("caliban")
        .join("output-styles");
    let custom_learning = "---\n\
name: learning\n\
description: \"USER OVERRIDE for learning.\"\n\
keep_coding_instructions: true\n\
---\n\
\n\
User-overridden body.\n";
    write_style(&user, "learning", custom_learning);
    let roots = roots_in(project, Some(user), None);
    let styles = load_styles(&roots);
    let learning = styles
        .iter()
        .find(|s| s.name == "learning")
        .expect("learning present");
    assert!(matches!(learning.source, OutputStyleSource::User { .. }));
    assert_eq!(learning.description, "USER OVERRIDE for learning.");
}

#[test]
fn plugin_styles_namespaced_in_listing() {
    let tmp = TempDir::new().unwrap();
    let project = tmp
        .path()
        .join("proj")
        .join(".caliban")
        .join("output-styles");
    let plugins_root = tmp.path().join("data").join("caliban").join("plugins");
    let plugin_styles = plugins_root.join("superpowers").join("output-styles");
    write_style(&plugin_styles, "tightlipped", TIGHTLIPPED);
    let roots = roots_in(project, None, Some(plugins_root));
    let styles = load_styles(&roots);
    let namespaced = styles
        .iter()
        .find(|s| s.name == "superpowers:tightlipped")
        .expect("plugin style namespaced");
    assert!(matches!(
        namespaced.source,
        OutputStyleSource::Plugin { .. }
    ));
}

// ---------------------------------------------------------------------------
// Selection (`select_active`)
// ---------------------------------------------------------------------------

#[test]
fn select_active_finds_known_name() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing"), None, None);
    let styles = load_styles(&roots);
    let chosen = select_active(&styles, "proactive", &[]).expect("found");
    assert_eq!(chosen.name, "proactive");
}

#[test]
fn select_active_falls_back_to_default_on_unknown_name() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing"), None, None);
    let styles = load_styles(&roots);
    let chosen = select_active(&styles, "does-not-exist", &[]).expect("default returns");
    assert_eq!(chosen.name, "default");
}

#[test]
fn force_for_plugin_overrides_when_plugin_enabled() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("missing-project");
    let plugins_root = tmp.path().join("data").join("caliban").join("plugins");
    let plugin_styles = plugins_root.join("superpowers").join("output-styles");
    write_style(&plugin_styles, "pinned", PLUGIN_FORCING);
    let roots = roots_in(project, None, Some(plugins_root));
    let styles = load_styles(&roots);
    let chosen = select_active(&styles, "proactive", &["superpowers".to_string()])
        .expect("plugin force wins");
    assert_eq!(chosen.name, "superpowers:pinned");
}

#[test]
fn force_for_plugin_ignored_when_plugin_disabled() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("missing-project");
    let plugins_root = tmp.path().join("data").join("caliban").join("plugins");
    let plugin_styles = plugins_root.join("superpowers").join("output-styles");
    write_style(&plugin_styles, "pinned", PLUGIN_FORCING);
    let roots = roots_in(project, None, Some(plugins_root));
    let styles = load_styles(&roots);
    let chosen = select_active(&styles, "proactive", &[]).expect("falls through");
    assert_eq!(chosen.name, "proactive");
}

#[test]
fn force_for_plugin_on_user_style_is_ignored() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("missing");
    let user = tmp
        .path()
        .join("user")
        .join("caliban")
        .join("output-styles");
    // Same body but loaded from user tier (not plugin tier).
    write_style(&user, "pinned", PLUGIN_FORCING);
    let roots = roots_in(project, Some(user), None);
    let styles = load_styles(&roots);
    // Even with `superpowers` in enabled plugins, a user-tier force_for_plugin
    // is ignored — selection follows `requested`.
    let chosen =
        select_active(&styles, "proactive", &["superpowers".to_string()]).expect("falls through");
    assert_eq!(chosen.name, "proactive");
}

// ---------------------------------------------------------------------------
// Splice
// ---------------------------------------------------------------------------

#[test]
fn splice_into_emits_xml_tag_with_name() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing"), None, None);
    let styles = load_styles(&roots);
    let learning = styles
        .iter()
        .find(|s| s.name == "learning")
        .unwrap()
        .clone();
    let prefix = OutputStylePrefix::new(Some(learning));
    let out = prefix.splice_into("BASE PROMPT BODY");
    assert!(
        out.contains("<output-style name=\"learning\">"),
        "missing open tag in:\n{out}"
    );
    assert!(out.contains("</output-style>"), "missing close tag");
    // Open tag must come before the BASE body.
    let open_idx = out.find("<output-style").unwrap();
    let base_idx = out.find("BASE PROMPT BODY").unwrap();
    assert!(open_idx < base_idx, "tag must precede base body");
}

#[test]
fn splice_into_default_returns_base_unchanged() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing"), None, None);
    let styles = load_styles(&roots);
    let default = styles.iter().find(|s| s.name == "default").unwrap().clone();
    let prefix = OutputStylePrefix::new(Some(default));
    let out = prefix.splice_into("BASE");
    assert_eq!(out, "BASE", "default style must be a no-op");
}

#[test]
fn splice_into_none_returns_base_unchanged() {
    let prefix = OutputStylePrefix::new(None);
    assert_eq!(prefix.splice_into("BASE"), "BASE");
}

#[test]
fn drops_coding_instructions_when_keep_is_false() {
    let tmp = TempDir::new().unwrap();
    let project = tmp
        .path()
        .join("proj")
        .join(".caliban")
        .join("output-styles");
    write_style(&project, "noco", NO_CODING);
    let roots = roots_in(project, None, None);
    let styles = load_styles(&roots);
    let noco = styles.iter().find(|s| s.name == "noco").unwrap().clone();
    let prefix = OutputStylePrefix::new(Some(noco));
    assert!(
        prefix.drops_coding_instructions(),
        "keep_coding_instructions=false must request dropping the coding block"
    );
}

#[test]
fn keep_coding_instructions_default_true_keeps_block() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing"), None, None);
    let styles = load_styles(&roots);
    let proactive = styles
        .iter()
        .find(|s| s.name == "proactive")
        .unwrap()
        .clone();
    let prefix = OutputStylePrefix::new(Some(proactive));
    assert!(
        !prefix.drops_coding_instructions(),
        "default keep_coding_instructions=true must keep the block"
    );
}

// ---------------------------------------------------------------------------
// Composition with MemoryPrefix-like base
// ---------------------------------------------------------------------------

#[test]
fn splice_into_inserts_between_memory_and_base_when_called_first() {
    // Verify the call-site composition: at the binary's splice point we
    // first wrap the base body with the output-style prefix, then wrap
    // *that* with the memory prefix. The final string therefore reads
    // memory → output-style → base, which is what the spec mandates.
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing"), None, None);
    let styles = load_styles(&roots);
    let learning = styles
        .iter()
        .find(|s| s.name == "learning")
        .unwrap()
        .clone();
    let style_prefix = OutputStylePrefix::new(Some(learning));

    let base = "BASE BODY";
    let with_style = style_prefix.splice_into(base);
    // The style block must precede the base in the inner string.
    let style_idx = with_style.find("<output-style").unwrap();
    let base_idx = with_style.find("BASE BODY").unwrap();
    assert!(style_idx < base_idx);

    // Now wrap with a MemoryPrefix-shaped outer block (simulated by
    // string concatenation — we don't pull in caliban-memory as a dev-dep
    // just for this test).
    let final_prompt =
        format!("<global-claude-md path=\"/g\">\nG\n</global-claude-md>\n\n{with_style}");
    let m_idx = final_prompt.find("<global-claude-md").unwrap();
    let s_idx = final_prompt.find("<output-style").unwrap();
    let b_idx = final_prompt.find("BASE BODY").unwrap();
    assert!(m_idx < s_idx, "memory must precede output-style");
    assert!(s_idx < b_idx, "output-style must precede base");
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

#[test]
fn registry_get_and_available() {
    let tmp = TempDir::new().unwrap();
    let roots = roots_in(tmp.path().join("missing"), None, None);
    let reg = OutputStylesRegistry::load_from(&roots);
    assert!(reg.get("default").is_some());
    assert!(reg.get("does-not-exist").is_none());
    let names: Vec<&str> = reg.available().iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"learning"));
}

// ---------------------------------------------------------------------------
// Learning post-processor
// ---------------------------------------------------------------------------

#[test]
fn learning_post_processor_inserts_todo_in_rust_fn() {
    let input = "Here is a function:\n\n\
        ```rust\n\
        fn compute(x: u32) -> u32 {\n\
        }\n\
        ```\n";
    let p = LearningPostProcessor::new();
    let out = p.process(input);
    assert!(
        out.contains("TODO(human)"),
        "learning post-processor should add TODO(human) — got:\n{out}"
    );
}

#[test]
fn learning_post_processor_inserts_todo_in_python_def() {
    let input = "Code:\n\n\
        ```python\n\
        def compute(x):\n\
        ```\n";
    let p = LearningPostProcessor::new();
    let out = p.process(input);
    assert!(out.contains("# TODO(human)"), "got:\n{out}");
}

#[test]
fn learning_post_processor_leaves_prose_alone() {
    let input = "Just some plain prose with no fenced code in it.\n";
    let p = LearningPostProcessor::new();
    let out = p.process(input);
    // Bytes round-trip exactly when no marker fires.
    assert_eq!(out.as_ref(), input);
}

#[test]
fn identity_post_processor_returns_input_unchanged() {
    let input = "anything here\nincluding `fn foo() {}` in prose\n";
    let p = caliban_output_styles::IdentityPostProcessor::new();
    let out = p.process(input);
    assert_eq!(out.as_ref(), input);
    assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
}

// ---------------------------------------------------------------------------
// Env-var selection
// ---------------------------------------------------------------------------

#[test]
fn env_var_selection_round_trip() {
    // Verify the env-var name is the constant we expect and that the
    // helper falls back to "default" when the var is absent. We avoid
    // mutating process env (tests share env state) — the selection wiring
    // is exercised end-to-end by `select_active` tests above.
    assert_eq!(
        caliban_output_styles::ACTIVE_STYLE_ENV,
        "CALIBAN_OUTPUT_STYLE"
    );
    // If the env var happens to be unset (the usual case), the helper
    // returns "default". If a caller of the test suite *did* set it,
    // we accept whatever they chose — that's a valid override.
    let from_env = caliban_output_styles::requested_from_env();
    if std::env::var("CALIBAN_OUTPUT_STYLE").ok().is_none() {
        assert_eq!(from_env, "default");
    }
}
