//! Static `ModelInfo` table for Google Gemini models.

use caliban_provider::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};

const fn caps(
    max_input: u32,
    max_output: u32,
    vision: bool,
    tools: bool,
    thinking: bool,
) -> Capabilities {
    let tool_use = if tools {
        ToolUseCapability::ParallelCalls
    } else {
        ToolUseCapability::None
    };
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: max_output,
        vision,
        tool_use,
        thinking,
        prompt_caching: PromptCachingCapability::None,
        json_mode: true,
        streaming: true,
        stop_sequences: true,
        top_k: true,
        system_prompt: SystemPromptCapability::SeparateField,
        refusal_field: false,
    }
}

/// Return the full list of known Google Gemini models.
#[must_use]
pub fn models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gemini-2.0-flash".into(),
            native_id: "gemini-2.0-flash".into(),
            display_name: "Gemini 2.0 Flash".into(),
            capabilities: caps(1_048_576, 8_192, true, true, false),
        },
        ModelInfo {
            id: "gemini-1.5-pro".into(),
            native_id: "gemini-1.5-pro".into(),
            display_name: "Gemini 1.5 Pro".into(),
            capabilities: caps(2_097_152, 8_192, true, true, false),
        },
        ModelInfo {
            id: "gemini-1.5-flash".into(),
            native_id: "gemini-1.5-flash".into(),
            display_name: "Gemini 1.5 Flash".into(),
            capabilities: caps(1_048_576, 8_192, true, true, false),
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
        .map_or_else(
            || caps(1_048_576, 8_192, false, false, false),
            |m| m.capabilities,
        )
}
