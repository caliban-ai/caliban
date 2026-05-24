//! `EnterPlanMode` and `ExitPlanMode` tools.
//!
//! See `docs/superpowers/specs/2026-05-23-plan-mode-design.md`.

use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use caliban_agent_core::{SharedPlanMode, Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct EnterInput {
    plan: String,
}

#[derive(Debug, Deserialize)]
struct ExitInput {
    #[serde(default = "default_true")]
    confirm: bool,
}

const fn default_true() -> bool {
    true
}

/// Set the session's plan-mode flag and echo the plan back to the model.
pub struct EnterPlanModeTool {
    handle: SharedPlanMode,
    schema: OnceLock<Value>,
}

impl std::fmt::Debug for EnterPlanModeTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnterPlanModeTool").finish_non_exhaustive()
    }
}

impl EnterPlanModeTool {
    /// Build the tool from a shared plan-mode handle.
    #[must_use]
    pub fn new(handle: SharedPlanMode) -> Self {
        Self {
            handle,
            schema: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &'static str {
        "EnterPlanMode"
    }

    fn description(&self) -> &'static str {
        "Enter plan mode and share your plan with the operator. While plan mode is active, \
         only read-only tools (Read, Grep, Glob, WebFetch, Skill, EnterPlanMode, ExitPlanMode) \
         run; mutating tools (Bash, Write, Edit, etc.) are rejected. The operator must exit \
         plan mode before any work begins. Pass the plan as numbered markdown steps."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "plan": {
                        "type": "string",
                        "description": "Markdown plan describing what you intend to do, in numbered steps."
                    }
                },
                "required": ["plan"]
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: EnterInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;
        self.handle.store(true, Ordering::Relaxed);
        let body = format!(
            "→ Plan mode entered. Operator must approve before tools that mutate state will run.\n\n{}",
            parsed.plan
        );
        Ok(vec![ContentBlock::Text(TextBlock {
            text: body,
            cache_control: None,
        })])
    }
}

/// Clear the plan-mode flag. The operator typically triggers this via the TUI;
/// model-initiated invocation is allowed but discouraged in v1.
pub struct ExitPlanModeTool {
    handle: SharedPlanMode,
    schema: OnceLock<Value>,
}

impl std::fmt::Debug for ExitPlanModeTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExitPlanModeTool").finish_non_exhaustive()
    }
}

impl ExitPlanModeTool {
    /// Build the tool from a shared plan-mode handle.
    #[must_use]
    pub fn new(handle: SharedPlanMode) -> Self {
        Self {
            handle,
            schema: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &'static str {
        "ExitPlanMode"
    }

    fn description(&self) -> &'static str {
        "Exit plan mode. Mutating tools become available again. The operator is the expected \
         caller via the /plan toggle; model-initiated exits should be rare and explicit."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "confirm": {
                        "type": "boolean",
                        "description": "Operator confirmation; rejected when false.",
                        "default": true
                    }
                }
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: ExitInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;
        if !parsed.confirm {
            return Err(ToolError::invalid_input(
                "ExitPlanMode requires confirm=true".to_string(),
            ));
        }
        self.handle.store(false, Ordering::Relaxed);
        Ok(vec![ContentBlock::Text(TextBlock {
            text: "Plan mode exited. Mutating tools are now available.".to_string(),
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::new_shared_plan_mode;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> ToolContext {
        ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn enter_plan_mode_sets_flag_and_echoes_plan() {
        let handle = new_shared_plan_mode();
        let tool = EnterPlanModeTool::new(handle.clone());
        let out = tool
            .invoke(json!({ "plan": "1. do x\n2. do y" }), ctx())
            .await
            .unwrap();
        assert!(handle.load(Ordering::Relaxed));
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected text block")
        };
        assert!(t.text.contains("Plan mode entered"));
        assert!(t.text.contains("1. do x"));
    }

    #[tokio::test]
    async fn exit_plan_mode_clears_flag() {
        let handle = new_shared_plan_mode();
        handle.store(true, Ordering::Relaxed);
        let tool = ExitPlanModeTool::new(handle.clone());
        let out = tool.invoke(json!({}), ctx()).await.unwrap();
        assert!(!handle.load(Ordering::Relaxed));
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("Plan mode exited"));
    }

    #[tokio::test]
    async fn exit_plan_mode_requires_confirm_true() {
        let handle = new_shared_plan_mode();
        handle.store(true, Ordering::Relaxed);
        let tool = ExitPlanModeTool::new(handle.clone());
        let err = tool
            .invoke(json!({ "confirm": false }), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
        // Flag must not change.
        assert!(handle.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn exit_plan_mode_default_confirm_is_true() {
        let handle = new_shared_plan_mode();
        handle.store(true, Ordering::Relaxed);
        let tool = ExitPlanModeTool::new(handle.clone());
        tool.invoke(json!({}), ctx()).await.unwrap();
        assert!(!handle.load(Ordering::Relaxed));
    }
}
