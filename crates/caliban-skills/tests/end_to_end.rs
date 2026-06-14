//! Integration tests for skill loading + the `SkillTool`.

use std::path::Path;

use caliban_agent_core::{ContentBlock, Tool, ToolContext, ToolError};
use caliban_skills::{Skill, SkillTool, load_skills, load_skills_report};
use tokio_util::sync::CancellationToken;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn well_formed(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: \"{description}\"\nmetadata:\n  trigger: pre-implementation\n---\n\n# {name}\n\nBody for {name}.\n",
    )
}

fn ctx() -> ToolContext {
    ToolContext {
        tool_use_id: "t1".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    }
}

#[test]
fn loads_well_formed_skill() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let skill_md = root.join("brainstorming/SKILL.md");
    write(
        &skill_md,
        &well_formed("brainstorming", "Use this before any creative work"),
    );

    let skills = load_skills(std::slice::from_ref(&root));
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "brainstorming");
    assert!(skills[0].description.contains("before any creative work"));
    assert!(skills[0].body.contains("Body for brainstorming"));
    assert_eq!(
        skills[0].metadata.get("trigger").and_then(|v| v.as_str()),
        Some("pre-implementation")
    );
}

#[test]
fn rejects_missing_frontmatter() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write(
        &root.join("plain/SKILL.md"),
        "no frontmatter here, just markdown.\n",
    );
    let skills = load_skills(&[root]);
    assert!(skills.is_empty());
}

#[test]
fn rejects_mismatched_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write(
        &root.join("foo/SKILL.md"),
        &well_formed("bar", "wrong name in frontmatter"),
    );
    let skills = load_skills(&[root]);
    assert!(skills.is_empty());
}

#[test]
fn report_records_mismatched_name_skip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let bad = root.join("foo/SKILL.md");
    write(&bad, &well_formed("bar", "wrong name in frontmatter"));

    let report = load_skills_report(std::slice::from_ref(&root));
    assert!(report.skills.is_empty());
    assert_eq!(report.skips.len(), 1);
    assert_eq!(report.skips[0].path, bad);
    assert!(
        report.skips[0]
            .reason
            .contains("does not match parent directory"),
        "reason should name the mismatch: {}",
        report.skips[0].reason
    );
}

#[test]
fn report_records_malformed_frontmatter_skip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write(
        &root.join("plain/SKILL.md"),
        "no frontmatter here, just markdown.\n",
    );
    let report = load_skills_report(&[root]);
    assert!(report.skills.is_empty());
    assert_eq!(report.skips.len(), 1);
    assert!(report.skips[0].reason.contains("frontmatter"));
}

#[test]
fn report_loads_valid_skill_and_skips_bad_one() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write(
        &root.join("good/SKILL.md"),
        &well_formed("good", "a valid skill"),
    );
    write(
        &root.join("oops/SKILL.md"),
        &well_formed("nope", "misnamed"),
    );

    let report = load_skills_report(&[root]);
    assert_eq!(report.skills.len(), 1);
    assert_eq!(report.skills[0].name, "good");
    assert_eq!(report.skips.len(), 1);
}

#[test]
fn report_shadowed_duplicate_is_not_a_skip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().join("workspace");
    let user = tmp.path().join("user");
    write(
        &workspace.join("brainstorming/SKILL.md"),
        &well_formed("brainstorming", "WORKSPACE version"),
    );
    write(
        &user.join("brainstorming/SKILL.md"),
        &well_formed("brainstorming", "USER version"),
    );

    let report = load_skills_report(&[workspace, user]);
    assert_eq!(report.skills.len(), 1);
    // Shadowing is intentional priority resolution, not a rejected file.
    assert!(report.skips.is_empty());
}

#[test]
fn priority_first_root_shadows_later() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().join("workspace");
    let user = tmp.path().join("user");
    write(
        &workspace.join("brainstorming/SKILL.md"),
        &well_formed("brainstorming", "WORKSPACE version"),
    );
    write(
        &user.join("brainstorming/SKILL.md"),
        &well_formed("brainstorming", "USER version"),
    );

    let skills = load_skills(&[workspace.clone(), user.clone()]);
    assert_eq!(skills.len(), 1);
    assert!(skills[0].description.contains("WORKSPACE"));
}

#[test]
fn missing_discovery_root_is_ok() {
    let skills = load_skills(&[std::path::PathBuf::from("/nonexistent/skills/root")]);
    assert!(skills.is_empty());
}

#[tokio::test]
async fn skill_tool_returns_body_on_match() {
    let tool = SkillTool::new(vec![Skill {
        name: "echo".into(),
        description: "echoes itself".into(),
        body: "BODY-HERE".into(),
        metadata: std::collections::BTreeMap::new(),
        source_path: "/tmp/echo/SKILL.md".into(),
    }]);
    let out = tool
        .invoke(serde_json::json!({"name": "echo"}), ctx())
        .await
        .unwrap();
    let ContentBlock::Text(t) = &out[0] else {
        panic!()
    };
    assert!(t.text.contains("BODY-HERE"));
    assert!(t.text.starts_with("→ Skill echo"));
}

#[tokio::test]
async fn skill_tool_invalid_input_on_miss() {
    let tool = SkillTool::new(vec![Skill {
        name: "alpha".into(),
        description: "first".into(),
        body: "x".into(),
        metadata: std::collections::BTreeMap::new(),
        source_path: "/tmp/alpha/SKILL.md".into(),
    }]);
    let err = tool
        .invoke(serde_json::json!({"name": "missing"}), ctx())
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
    let msg = format!("{err}");
    assert!(msg.contains("no skill named 'missing'"));
    assert!(msg.contains("alpha"));
}

#[tokio::test]
async fn skill_tool_description_lists_all_skills() {
    let tool = SkillTool::new(vec![
        Skill {
            name: "alpha".into(),
            description: "first".into(),
            body: "x".into(),
            metadata: std::collections::BTreeMap::new(),
            source_path: "/tmp/alpha/SKILL.md".into(),
        },
        Skill {
            name: "beta".into(),
            description: "second".into(),
            body: "y".into(),
            metadata: std::collections::BTreeMap::new(),
            source_path: "/tmp/beta/SKILL.md".into(),
        },
    ]);
    let desc = tool.description();
    assert!(desc.contains("alpha"));
    assert!(desc.contains("beta"));
}

#[tokio::test]
async fn empty_skill_tool_has_no_skills_in_description() {
    let tool = SkillTool::new(Vec::new());
    let desc = tool.description();
    assert!(desc.contains("no skills are currently loaded"));
    assert!(tool.is_empty());
}

#[test]
fn metadata_passthrough() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write(
        &root.join("custom/SKILL.md"),
        "---\nname: custom\ndescription: \"a skill\"\nmetadata:\n  cost: low\n  tags:\n    - one\n    - two\n---\nbody\n",
    );
    let skills = load_skills(&[root]);
    assert_eq!(skills.len(), 1);
    let s = &skills[0];
    assert_eq!(s.metadata.get("cost").and_then(|v| v.as_str()), Some("low"));
    let tags = s.metadata.get("tags").unwrap();
    assert!(tags.is_sequence());
}
