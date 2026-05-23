//! Capability discovery types.

use serde::{Deserialize, Serialize};

/// The full set of capabilities offered by a model/provider combination.
// The many boolean fields are intentional; each represents a distinct yes/no capability.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capabilities {
    /// Maximum number of input tokens supported.
    pub max_input_tokens: u32,
    /// Maximum number of output tokens supported.
    pub max_output_tokens: u32,
    /// Whether the model accepts image inputs.
    pub vision: bool,
    /// The level of tool-use support.
    pub tool_use: ToolUseCapability,
    /// Whether extended thinking is supported.
    pub thinking: bool,
    /// The level of prompt-caching support.
    pub prompt_caching: PromptCachingCapability,
    /// Whether structured JSON output mode is supported.
    pub json_mode: bool,
    /// Whether token streaming is supported.
    pub streaming: bool,
    /// Whether stop sequences are supported.
    pub stop_sequences: bool,
    /// Whether top-k sampling is supported.
    pub top_k: bool,
    /// How system prompts are passed to this model.
    pub system_prompt: SystemPromptCapability,
    /// Whether the provider returns a structured refusal field.
    pub refusal_field: bool,
}

/// Level of tool-use support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolUseCapability {
    /// Tool use is not supported.
    None,
    /// A single tool call per turn.
    Basic,
    /// Multiple parallel tool calls per turn.
    ParallelCalls,
}

/// Level of prompt-caching support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptCachingCapability {
    /// No prompt caching.
    None,
    /// The provider caches automatically without explicit markers.
    Automatic,
    /// The caller must mark breakpoints; carries the maximum number.
    Explicit {
        /// Maximum number of cache breakpoints allowed.
        max_breakpoints: u32,
    },
}

/// How the model/provider expects system prompts to be passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemPromptCapability {
    /// Provider has a dedicated top-level `system` field.
    SeparateField,
    /// System prompts are passed as a message with `role: "system"`.
    SystemRole,
    /// System prompts are passed as a message with `role: "developer"`.
    DeveloperRole,
}

/// Metadata for a single model variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Caliban-internal model identifier.
    pub id: String,
    /// The model identifier used in API requests.
    pub native_id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// The capabilities of this model.
    pub capabilities: Capabilities,
}
