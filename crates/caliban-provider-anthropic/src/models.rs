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
#[must_use]
pub fn models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-3-5-sonnet".into(),
            native_id: "claude-3-5-sonnet-20241022".into(),
            display_name: "Claude 3.5 Sonnet".into(),
            capabilities: caps(200_000, 8_192, true, false),
        },
        ModelInfo {
            id: "claude-3-5-haiku".into(),
            native_id: "claude-3-5-haiku-20241022".into(),
            display_name: "Claude 3.5 Haiku".into(),
            capabilities: caps(200_000, 8_192, true, false),
        },
        ModelInfo {
            id: "claude-3-opus".into(),
            native_id: "claude-3-opus-20240229".into(),
            display_name: "Claude 3 Opus".into(),
            capabilities: caps(200_000, 4_096, true, false),
        },
        ModelInfo {
            id: "claude-3-haiku".into(),
            native_id: "claude-3-haiku-20240307".into(),
            display_name: "Claude 3 Haiku".into(),
            capabilities: caps(200_000, 4_096, true, false),
        },
        ModelInfo {
            id: "claude-3-7-sonnet".into(),
            native_id: "claude-3-7-sonnet-20250219".into(),
            display_name: "Claude 3.7 Sonnet".into(),
            capabilities: caps(200_000, 8_192, true, true),
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
