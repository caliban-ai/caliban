//! `AgentTool` — spawns an in-process sub-agent with a restricted tool palette.
//!
//! See `docs/superpowers/specs/2026-05-23-sub-agent-design.md` and
//! ADR 0037 (worktree isolation + background fleet additions).

use std::path::PathBuf;
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

/// Isolation mode for a spawned sub-agent (ADR 0037).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    /// Run in the parent's working directory (today's behavior).
    #[default]
    None,
    /// Materialize a dedicated git worktree under `.caliban/worktrees/<name>`
    /// and run the sub-agent there.
    Worktree,
}

/// Optional worktree settings — only consulted when `isolation =
/// Worktree`. Mirrors `caliban_worktrees::WorktreeSpec` so it can be
/// passed straight through.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorktreeOptions {
    /// Base ref to root the worktree on. Defaults to `head`. Valid
    /// values: `fresh`, `head`, or a rev-parse-able ref string.
    #[serde(default)]
    pub base_ref: Option<String>,
    /// Sparse-checkout patterns.
    #[serde(default)]
    pub sparse_paths: Vec<String>,
    /// Symlink these dirs (relative to parent repo root) into the worktree.
    #[serde(default)]
    pub symlink_directories: Vec<PathBuf>,
}

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
    /// Isolation mode. Defaults to [`IsolationMode::None`].
    #[serde(default)]
    pub isolation: IsolationMode,
    /// True iff the sub-agent should run detached (handed to the
    /// supervisor daemon). Defaults to `false`.
    #[serde(default)]
    pub background: bool,
    /// Inherit parent hooks. Defaults to `true`; opt-out for sub-agents
    /// that must run with a fresh chain.
    #[serde(default = "default_inherit_hooks")]
    pub inherit_hooks: bool,
    /// Inherit the parent's active MCP set (ADR-0046). Defaults to
    /// `true`. When false the child starts with an empty activation
    /// set and must call `ToolSearch` to populate it. Independent of
    /// `tool_allowlist`: the allowlist runs first, then activation
    /// inheritance only affects MCP tools that survive the allowlist.
    #[serde(default = "default_inherit_active_mcp")]
    pub inherit_active_mcp: bool,
    /// Optional human-readable label that appears in `/agents` and logs.
    #[serde(default)]
    pub label: Option<String>,
    /// Worktree options (only honored when `isolation == Worktree`).
    #[serde(default)]
    pub worktree: Option<WorktreeOptions>,
}

fn default_inherit_hooks() -> bool {
    true
}

fn default_inherit_active_mcp() -> bool {
    true
}

/// Factory closure passed to [`AgentTool::new`]. Given the parsed input, it
/// returns a freshly-configured sub-`Agent` (filtered registry + chosen
/// model + parent's provider/hooks).
pub type AgentFactory = Arc<dyn Fn(&AgentToolInput) -> Agent + Send + Sync>;

/// Background-handoff hook installed by the caliban binary. When the
/// parent receives `background: true`, the tool calls this with the
/// parsed input + sub-agent and expects back an opaque id + per-agent
/// socket path. Returning `Err` falls back to the foreground path.
///
/// We use a trait-object closure (instead of `tokio::sync::mpsc` or a
/// dedicated trait) to keep the dependency surface tiny — the
/// `AgentTool` doesn't need to depend on `caliban-supervisor` directly.
pub type BackgroundSpawner = Arc<dyn Fn(&AgentToolInput) -> BackgroundSpawnResult + Send + Sync>;

/// Outcome of a background-handoff attempt.
#[derive(Debug, Clone)]
pub struct BackgroundSpawnResult {
    /// Newly assigned agent id.
    pub id: String,
    /// Per-agent socket the user can `caliban attach <id>` to.
    pub socket_path: PathBuf,
}

/// Built-in `AgentTool` that lets the parent agent spawn a synchronous
/// (or, with `background: true`, detached) sub-agent.
pub struct AgentTool {
    factory: AgentFactory,
    parent_system_prompt: Option<String>,
    schema: OnceLock<Value>,
    background_spawner: Option<BackgroundSpawner>,
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
            background_spawner: None,
        }
    }

    /// Install a background-handoff spawner. Without one, `background:
    /// true` falls back to foreground execution with a warning.
    #[must_use]
    pub fn with_background_spawner(mut self, spawner: BackgroundSpawner) -> Self {
        self.background_spawner = Some(spawner);
        self
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
                    },
                    "isolation": {
                        "type": "string",
                        "enum": ["none", "worktree"],
                        "description": "Isolation mode. `worktree` materializes a dedicated git worktree under .caliban/worktrees/<name> and runs the sub-agent there. Defaults to `none`."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "If true, hand the sub-agent off to the supervisor daemon and return its id immediately. Defaults to false."
                    },
                    "inherit_hooks": {
                        "type": "boolean",
                        "description": "Whether the sub-agent inherits the parent's Hooks chain. Defaults to true; closures cannot cross the process boundary for background spawns and are dropped with a warning."
                    },
                    "label": {
                        "type": ["string", "null"],
                        "description": "Optional human-readable label surfaced in `/agents` and logs."
                    },
                    "worktree": {
                        "type": ["object", "null"],
                        "description": "Worktree settings; only consulted when isolation=worktree.",
                        "properties": {
                            "base_ref": { "type": ["string", "null"] },
                            "sparse_paths": { "type": "array", "items": { "type": "string" } },
                            "symlink_directories": { "type": "array", "items": { "type": "string" } }
                        }
                    }
                },
                "required": ["prompt"]
            })
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: AgentToolInput = crate::parse_input(input)?;

        // Background handoff path (ADR 0037). When the operator requests
        // `background: true` and the binary installed a spawner, we
        // delegate to the supervisor and return an opaque id. Closure
        // hooks can't cross the process boundary; warn loudly and drop
        // them so the daemon's reconstructed chain matches reality.
        if parsed.background {
            if parsed.inherit_hooks && cx.hooks.is_some() {
                tracing::warn!(
                    "AgentTool: dropping closure-based parent hooks for background sub-agent \
                     (closures cannot cross the process boundary); only config-expressible \
                     hooks survive. Pass `inherit_hooks: false` to silence this warning."
                );
            }
            if let Some(spawn) = &self.background_spawner {
                let outcome = spawn(&parsed);
                let label = parsed
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("agent-{}", outcome.id));
                let text = format!(
                    "[backgrounded sub-agent {} ({}); attach via `caliban attach {}` or the /agents overlay]\nsocket: {}",
                    outcome.id,
                    label,
                    outcome.id,
                    outcome.socket_path.display(),
                );
                return Ok(vec![ContentBlock::Text(TextBlock {
                    text,
                    cache_control: None,
                })]);
            }
            tracing::warn!(
                "AgentTool: background=true requested but no supervisor spawner installed; \
                 falling back to foreground execution."
            );
        }

        let agent_name_for_hook = parsed.model.clone().unwrap_or_default();
        let task_id_for_hook = cx.tool_use_id.clone();
        let parent_turn_index = cx.turn_index;
        let sub_agent = (self.factory)(&parsed);
        let sub_agent = Arc::new(sub_agent);

        // Fire SubagentStart (best-effort).
        if let Some(hooks) = cx.hooks.as_ref() {
            let sub_ctx = caliban_agent_core::SubagentCtx {
                parent_turn_index,
                agent_name: &agent_name_for_hook,
                task_id: &task_id_for_hook,
            };
            if let Err(e) = hooks.subagent_start(&sub_ctx).await {
                tracing::warn!(error = %e, "subagent_start hook error (non-fatal)");
            }
        }

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

        // Fire SubagentStop (best-effort).
        if let Some(hooks) = cx.hooks.as_ref() {
            let sub_ctx = caliban_agent_core::SubagentCtx {
                parent_turn_index,
                agent_name: &agent_name_for_hook,
                task_id: &task_id_for_hook,
            };
            let outcome = caliban_agent_core::SubagentOutcome {
                success: !hit_max,
                final_text: output.clone(),
            };
            if let Err(e) = hooks.subagent_stop(&sub_ctx, &outcome).await {
                tracing::warn!(error = %e, "subagent_stop hook error (non-fatal)");
            }
        }

        Ok(vec![ContentBlock::Text(TextBlock {
            text: output,
            cache_control: None,
        })])
    }
}

#[doc(hidden)]
pub const __SUB_AGENT_MAX_TURNS: u32 = SUB_AGENT_MAX_TURNS;
