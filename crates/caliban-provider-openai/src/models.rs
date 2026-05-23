//! Static `ModelInfo` table for `OpenAI` models.

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
        prompt_caching: PromptCachingCapability::Automatic,
        json_mode: true,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: SystemPromptCapability::SystemRole,
        refusal_field: true,
    }
}

/// Like [`caps`] but advertises `DeveloperRole` for o1-series models.
const fn caps_o1(max_input: u32, max_output: u32, vision: bool, thinking: bool) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: max_output,
        vision,
        tool_use: ToolUseCapability::ParallelCalls,
        thinking,
        prompt_caching: PromptCachingCapability::Automatic,
        json_mode: true,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: SystemPromptCapability::DeveloperRole,
        refusal_field: true,
    }
}

/// Return the full list of known `OpenAI` models.
#[must_use]
pub fn models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gpt-4o".into(),
            native_id: "gpt-4o".into(),
            display_name: "GPT-4o".into(),
            capabilities: caps(128_000, 4_096, true, false),
        },
        ModelInfo {
            id: "gpt-4o-mini".into(),
            native_id: "gpt-4o-mini".into(),
            display_name: "GPT-4o mini".into(),
            capabilities: caps(128_000, 16_384, true, false),
        },
        ModelInfo {
            id: "o1-preview".into(),
            native_id: "o1-preview".into(),
            display_name: "o1 preview".into(),
            capabilities: caps_o1(128_000, 32_768, false, true),
        },
        ModelInfo {
            id: "o1-mini".into(),
            native_id: "o1-mini".into(),
            display_name: "o1 mini".into(),
            capabilities: caps_o1(128_000, 65_536, false, true),
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
        .map_or_else(|| caps(128_000, 4_096, false, false), |m| m.capabilities)
}
