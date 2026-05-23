//! Static `ModelInfo` table for Ollama models.

use caliban_provider::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};

const fn caps(max_input: u32, max_output: u32, vision: bool, tool_use: bool) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: max_output,
        vision,
        tool_use: if tool_use {
            ToolUseCapability::Basic
        } else {
            ToolUseCapability::None
        },
        thinking: false,
        prompt_caching: PromptCachingCapability::None,
        json_mode: true,
        streaming: true,
        stop_sequences: true,
        top_k: true,
        system_prompt: SystemPromptCapability::SystemRole,
        refusal_field: false,
    }
}

/// Return the full list of known Ollama models.
#[must_use]
pub fn models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "llama3.1".into(),
            native_id: "llama3.1".into(),
            display_name: "Llama 3.1 (Ollama)".into(),
            capabilities: caps(128_000, 4096, false, true),
        },
        ModelInfo {
            id: "qwen2.5".into(),
            native_id: "qwen2.5".into(),
            display_name: "Qwen 2.5 (Ollama)".into(),
            capabilities: caps(32_768, 4096, false, true),
        },
        ModelInfo {
            id: "mistral".into(),
            native_id: "mistral".into(),
            display_name: "Mistral (Ollama)".into(),
            capabilities: caps(32_768, 4096, false, true),
        },
        ModelInfo {
            id: "phi3".into(),
            native_id: "phi3".into(),
            display_name: "Phi 3 (Ollama)".into(),
            capabilities: caps(4_096, 4096, false, false),
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
        .map_or_else(|| caps(8_192, 4096, false, false), |m| m.capabilities)
}
