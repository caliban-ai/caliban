//! `AgentTool` — spawns an in-process sub-agent with a restricted tool palette.
//!
//! See `docs/superpowers/specs/2026-05-23-sub-agent-design.md`.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Agent, ContentBlock, Message, Tool, ToolContext, ToolError, TurnEvent};
use caliban_provider::TextBlock;
use futures::StreamExt as _;
use serde::Deserialize;
use serde_json::{Value, json};

/// Hard limit on the sub-agent's final-text payload returned to the parent.
const MAX_OUTPUT_CHARS: usize = 5_000;

/// Hard turn limit for sub-agents.
const SUB_AGENT_MAX_TURNS: u32 = 20;

/// Parsed input shape for [`AgentTool`].
#[derive(Debug, Deserialize)]
pub struct AgentToolInput {
    /// Task description handed to the sub-agent as its first user message.
    pub prompt: String,
    /// Optional tool-name allowlist. `None` means inherit all parent tools
    /// except `AgentTool` itself.
    #[serde(default)]
    pub tool_allowlist: Option<Vec<String>>,
    /// Optional model override. `None` inherits the parent's model.
    #[serde(default)]
    pub model: Option<String>,
}

/// Factory closure passed to [`AgentTool::new`]. Given the parsed input, it
/// returns a freshly-configured sub-`Agent` (filtered registry + chosen
/// model + parent's provider/hooks).
pub type AgentFactory = Arc<dyn Fn(&AgentToolInput) -> Agent + Send + Sync>;

/// Built-in `AgentTool` that lets the parent agent spawn a synchronous
/// sub-agent.
pub struct AgentTool {
    factory: AgentFactory,
    parent_system_prompt: Option<String>,
    schema: OnceLock<Value>,
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field(
                "parent_system_prompt",
                &self
                    .parent_system_prompt
                    .as_deref()
                    .map(|s| &s[..s.len().min(40)]),
            )
            .finish_non_exhaustive()
    }
}

impl AgentTool {
    /// Build the tool from a factory closure and the parent's system prompt
    /// (which is replayed as the sub-agent's system message).
    #[must_use]
    pub fn new(factory: AgentFactory, parent_system_prompt: Option<String>) -> Self {
        Self {
            factory,
            parent_system_prompt,
            schema: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &'static str {
        "AgentTool"
    }

    fn description(&self) -> &'static str {
        "Spawn a synchronous sub-agent with a restricted tool palette. Returns the sub-agent's \
         final text. Use this to (a) run multi-step investigations without polluting the parent \
         transcript, or (b) restrict a subtask to read-only tools (Read, Grep, Glob). \
         Set `tool_allowlist` to a list of tool names; omit it to inherit all parent tools \
         except AgentTool itself. Sub-agents cannot recurse."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task description handed to the sub-agent as its first user message."
                    },
                    "tool_allowlist": {
                        "type": ["array", "null"],
                        "items": { "type": "string" },
                        "description": "Names of tools the sub-agent may use. If null or omitted, inherits all parent tools except AgentTool itself."
                    },
                    "model": {
                        "type": ["string", "null"],
                        "description": "Optional model id override. If null, inherits the parent's model."
                    }
                },
                "required": ["prompt"]
            })
        })
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: AgentToolInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;

        let sub_agent = (self.factory)(&parsed);
        let sub_agent = Arc::new(sub_agent);

        let mut initial: Vec<Message> = Vec::with_capacity(2);
        if let Some(sp) = &self.parent_system_prompt {
            initial.push(Message::system_text(sp.clone()));
        }
        initial.push(Message::user_text(parsed.prompt));

        let child_cancel = cx.cancel.child_token();
        let mut stream = Arc::clone(&sub_agent).stream_until_done(initial, child_cancel);

        let mut last_assistant_text = String::new();
        let mut hit_max = false;
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(TurnEvent::TurnEnd {
                    assistant_message, ..
                }) => {
                    let buf: String = assistant_message
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            ContentBlock::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !buf.is_empty() {
                        last_assistant_text = buf;
                    }
                }
                Ok(TurnEvent::RunEnd { stopped_for, .. }) => {
                    use caliban_agent_core::StopCondition;
                    if matches!(stopped_for, StopCondition::MaxTurnsReached(_)) {
                        hit_max = true;
                    }
                    if matches!(stopped_for, StopCondition::Cancelled) {
                        return Err(ToolError::Cancelled);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(ToolError::execution(std::io::Error::other(format!(
                        "sub-agent error: {e}"
                    ))));
                }
            }
        }

        let mut output = if hit_max {
            format!("[sub-agent exhausted max_turns without completing]\n\n{last_assistant_text}")
        } else {
            last_assistant_text
        };

        if output.chars().count() > MAX_OUTPUT_CHARS {
            let truncated: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
            output = format!("{truncated}\n\n[sub-agent output truncated]");
        }

        Ok(vec![ContentBlock::Text(TextBlock {
            text: output,
            cache_control: None,
        })])
    }
}

#[doc(hidden)]
pub const __SUB_AGENT_MAX_TURNS: u32 = SUB_AGENT_MAX_TURNS;
