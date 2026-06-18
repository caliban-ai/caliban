//! `SkillTool` — Tool that exposes loaded skills to the model.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::skill::Skill;

const DESCRIPTION_BUDGET_BYTES: usize = 8 * 1024;

#[derive(Debug, Deserialize)]
struct SkillInput {
    name: String,
}

/// Built-in tool that loads a skill's instruction set into the model's
/// context. The model invokes it by exact `name`.
pub struct SkillTool {
    skills: HashMap<String, Skill>,
    description: String,
    schema: OnceLock<Value>,
}

impl std::fmt::Debug for SkillTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillTool")
            .field("loaded", &self.skills.len())
            .finish_non_exhaustive()
    }
}

impl SkillTool {
    /// Build a `SkillTool` from a list of loaded skills.
    #[must_use]
    pub fn new(skills: Vec<Skill>) -> Self {
        let map: HashMap<String, Skill> = skills.into_iter().map(|s| (s.name.clone(), s)).collect();
        let description = build_description(&map);
        Self {
            skills: map,
            description,
            schema: OnceLock::new(),
        }
    }

    /// Read-only view of all loaded skills, keyed by `name`.
    #[must_use]
    pub fn skills(&self) -> &HashMap<String, Skill> {
        &self.skills
    }

    /// Number of loaded skills.
    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the loader produced no skills.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Loaded skill names, sorted for stable output. Used to surface a
    /// compact "skills awareness" section in the system prompt so the model
    /// proactively invokes a matching skill instead of improvising.
    #[must_use]
    pub fn skill_names_sorted(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.skills.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

fn build_description(skills: &HashMap<String, Skill>) -> String {
    let intro = "Loads a skill's instruction set. Call with the exact skill name to receive its body as text, then follow the instructions. If a listed skill matches the task at hand, invoke it before improvising.";
    if skills.is_empty() {
        return format!("{intro} (no skills are currently loaded)");
    }

    // Sort by name for stable output.
    let mut names: Vec<&str> = skills.keys().map(String::as_str).collect();
    names.sort_unstable();

    let header = format!("{intro} Available skills:\n");
    let mut body = String::new();
    let mut overflow = false;

    for name in &names {
        let skill = &skills[*name];
        let first_line = skill
            .description
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let entry = format!("- {name}: {first_line}\n");
        if header.len() + body.len() + entry.len() > DESCRIPTION_BUDGET_BYTES {
            overflow = true;
            // Truncate this entry's description if there's room for a stub.
            let prefix = format!("- {name}: ");
            let remaining = DESCRIPTION_BUDGET_BYTES
                .saturating_sub(header.len() + body.len() + prefix.len() + 32);
            if remaining > 0 {
                let truncated: String = first_line.chars().take(remaining / 2).collect();
                let _ = writeln!(body, "{prefix}{truncated}… (description truncated)");
            }
            break;
        }
        body.push_str(&entry);
    }

    if overflow && body.len() < DESCRIPTION_BUDGET_BYTES {
        // Already appended truncation marker on the last fitted line.
    }
    format!("{header}\n{body}")
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "Skill"
    }

    // The Skill tool only injects skill context into the conversation; it has
    // no workspace side effects, so it is plan-mode-safe.
    fn is_read_only(&self) -> bool {
        true
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Exact name of the skill to load (case-sensitive)."
                    }
                },
                "required": ["name"]
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: SkillInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;
        let Some(skill) = self.skills.get(&parsed.name) else {
            let mut available: Vec<&str> = self.skills.keys().map(String::as_str).collect();
            available.sort_unstable();
            let preview = available
                .iter()
                .take(10)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ToolError::invalid_input(format!(
                "no skill named '{}' (available: {})",
                parsed.name, preview
            )));
        };
        let text = format!("→ Skill {}\n\n{}", skill.name, skill.body);
        Ok(vec![ContentBlock::Text(TextBlock {
            text,
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, desc: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.to_string(),
            body: format!("body of {name}"),
            metadata: std::collections::BTreeMap::new(),
            source_path: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn description_nudges_proactive_invocation() {
        let tool = SkillTool::new(vec![skill("brainstorming", "Use before creative work")]);
        assert!(
            tool.description().contains("invoke it before improvising"),
            "description should nudge proactive invocation: {}",
            tool.description()
        );
    }

    #[test]
    fn description_respects_budget() {
        // Many skills with long descriptions must not exceed the budget.
        let skills: Vec<Skill> = (0..500)
            .map(|i| skill(&format!("skill-{i:04}"), &"x".repeat(200)))
            .collect();
        let tool = SkillTool::new(skills);
        assert!(tool.description().len() <= DESCRIPTION_BUDGET_BYTES);
    }

    #[test]
    fn skill_names_sorted_returns_sorted_names() {
        let tool = SkillTool::new(vec![
            skill("zeta", "z"),
            skill("alpha", "a"),
            skill("mid", "m"),
        ]);
        assert_eq!(tool.skill_names_sorted(), vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn as_any_downcasts_to_skill_tool() {
        let tool = SkillTool::new(vec![skill("alpha", "a")]);
        let any = Tool::as_any(&tool).expect("SkillTool overrides as_any");
        assert!(any.downcast_ref::<SkillTool>().is_some());
    }
}
