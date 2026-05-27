//! Static `ModelInfo` table for Anthropic Claude.

use caliban_provider::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};

const fn caps(max_input: u32, max_output: u32, vision: bool, thinking: bool) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: max_output,
        vision,
        tool_use: ToolUseCapability::ParallelCalls,
        thinking,
        prompt_caching: PromptCachingCapability::Explicit { max_breakpoints: 4 },
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: true,
        system_prompt: SystemPromptCapability::SeparateField,
        refusal_field: true,
    }
}

/// Return the full list of known Anthropic Claude models.
///
/// Sourced from <https://docs.anthropic.com/en/docs/about-claude/models>.
/// All Claude 3.x snapshots were retired by Feb 2026; only the Claude 4.x
/// family remains active.
#[must_use]
pub fn models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-opus-4-7".into(),
            native_id: "claude-opus-4-7".into(),
            display_name: "Claude Opus 4.7".into(),
            capabilities: caps(200_000, 32_000, true, true),
        },
        ModelInfo {
            id: "claude-sonnet-4-6".into(),
            native_id: "claude-sonnet-4-6".into(),
            display_name: "Claude Sonnet 4.6".into(),
            capabilities: caps(200_000, 64_000, true, true),
        },
        ModelInfo {
            id: "claude-haiku-4-5".into(),
            native_id: "claude-haiku-4-5".into(),
            display_name: "Claude Haiku 4.5".into(),
            capabilities: caps(200_000, 32_000, true, true),
        },
    ]
}

/// Look up `Capabilities` for a model by canonical or native ID.
///
/// Falls back to conservative defaults if the model is not in the table.
#[must_use]
pub fn capabilities_for(model: &str) -> Capabilities {
    models()
        .into_iter()
        .find(|m| m.id == model || m.native_id == model)
        .map_or_else(|| caps(100_000, 4_096, false, false), |m| m.capabilities)
}
