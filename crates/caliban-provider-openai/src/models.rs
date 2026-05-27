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
///
/// Mirrors the chat / reasoning entries returned by `GET /v1/models` (live API
/// snapshot taken 2026-05-27). Non-text modalities (TTS, transcribe,
/// embeddings, image, audio, realtime, sora, moderation) and pinned date
/// snapshots are intentionally omitted; canonical aliases cover their
/// behavior.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn models() -> Vec<ModelInfo> {
    vec![
        // GPT-5.5 — current flagship
        ModelInfo {
            id: "gpt-5.5".into(),
            native_id: "gpt-5.5".into(),
            display_name: "GPT-5.5".into(),
            capabilities: caps(1_000_000, 64_000, true, false),
        },
        ModelInfo {
            id: "gpt-5.5-pro".into(),
            native_id: "gpt-5.5-pro".into(),
            display_name: "GPT-5.5 Pro".into(),
            capabilities: caps(1_000_000, 128_000, true, true),
        },
        // GPT-5.4
        ModelInfo {
            id: "gpt-5.4".into(),
            native_id: "gpt-5.4".into(),
            display_name: "GPT-5.4".into(),
            capabilities: caps(1_000_000, 64_000, true, false),
        },
        ModelInfo {
            id: "gpt-5.4-pro".into(),
            native_id: "gpt-5.4-pro".into(),
            display_name: "GPT-5.4 Pro".into(),
            capabilities: caps(1_000_000, 128_000, true, true),
        },
        ModelInfo {
            id: "gpt-5.4-mini".into(),
            native_id: "gpt-5.4-mini".into(),
            display_name: "GPT-5.4 mini".into(),
            capabilities: caps(1_000_000, 64_000, true, false),
        },
        ModelInfo {
            id: "gpt-5.4-nano".into(),
            native_id: "gpt-5.4-nano".into(),
            display_name: "GPT-5.4 nano".into(),
            capabilities: caps(1_000_000, 64_000, true, false),
        },
        // GPT-5.3 — Codex-only release
        ModelInfo {
            id: "gpt-5.3-codex".into(),
            native_id: "gpt-5.3-codex".into(),
            display_name: "GPT-5.3 Codex".into(),
            capabilities: caps(400_000, 128_000, true, true),
        },
        // GPT-5.2
        ModelInfo {
            id: "gpt-5.2".into(),
            native_id: "gpt-5.2".into(),
            display_name: "GPT-5.2".into(),
            capabilities: caps(400_000, 128_000, true, false),
        },
        ModelInfo {
            id: "gpt-5.2-pro".into(),
            native_id: "gpt-5.2-pro".into(),
            display_name: "GPT-5.2 Pro".into(),
            capabilities: caps(400_000, 128_000, true, true),
        },
        ModelInfo {
            id: "gpt-5.2-codex".into(),
            native_id: "gpt-5.2-codex".into(),
            display_name: "GPT-5.2 Codex".into(),
            capabilities: caps(400_000, 128_000, true, true),
        },
        // GPT-5.1
        ModelInfo {
            id: "gpt-5.1".into(),
            native_id: "gpt-5.1".into(),
            display_name: "GPT-5.1".into(),
            capabilities: caps(400_000, 128_000, true, false),
        },
        ModelInfo {
            id: "gpt-5.1-codex".into(),
            native_id: "gpt-5.1-codex".into(),
            display_name: "GPT-5.1 Codex".into(),
            capabilities: caps(400_000, 128_000, true, true),
        },
        ModelInfo {
            id: "gpt-5.1-codex-mini".into(),
            native_id: "gpt-5.1-codex-mini".into(),
            display_name: "GPT-5.1 Codex mini".into(),
            capabilities: caps(400_000, 128_000, true, true),
        },
        ModelInfo {
            id: "gpt-5.1-codex-max".into(),
            native_id: "gpt-5.1-codex-max".into(),
            display_name: "GPT-5.1 Codex max".into(),
            capabilities: caps(400_000, 200_000, true, true),
        },
        // GPT-5 base family
        ModelInfo {
            id: "gpt-5".into(),
            native_id: "gpt-5".into(),
            display_name: "GPT-5".into(),
            capabilities: caps(400_000, 128_000, true, false),
        },
        ModelInfo {
            id: "gpt-5-mini".into(),
            native_id: "gpt-5-mini".into(),
            display_name: "GPT-5 mini".into(),
            capabilities: caps(400_000, 128_000, true, false),
        },
        ModelInfo {
            id: "gpt-5-nano".into(),
            native_id: "gpt-5-nano".into(),
            display_name: "GPT-5 nano".into(),
            capabilities: caps(400_000, 128_000, true, false),
        },
        ModelInfo {
            id: "gpt-5-pro".into(),
            native_id: "gpt-5-pro".into(),
            display_name: "GPT-5 Pro".into(),
            capabilities: caps(400_000, 128_000, true, true),
        },
        ModelInfo {
            id: "gpt-5-codex".into(),
            native_id: "gpt-5-codex".into(),
            display_name: "GPT-5 Codex".into(),
            capabilities: caps(400_000, 128_000, true, true),
        },
        // Reasoning (o-series)
        ModelInfo {
            id: "o1".into(),
            native_id: "o1".into(),
            display_name: "o1".into(),
            capabilities: caps_o1(200_000, 100_000, true, true),
        },
        ModelInfo {
            id: "o1-pro".into(),
            native_id: "o1-pro".into(),
            display_name: "o1 pro".into(),
            capabilities: caps_o1(200_000, 100_000, true, true),
        },
        ModelInfo {
            id: "o3".into(),
            native_id: "o3".into(),
            display_name: "o3".into(),
            capabilities: caps_o1(200_000, 100_000, true, true),
        },
        ModelInfo {
            id: "o3-mini".into(),
            native_id: "o3-mini".into(),
            display_name: "o3 mini".into(),
            capabilities: caps_o1(200_000, 100_000, false, true),
        },
        ModelInfo {
            id: "o3-pro".into(),
            native_id: "o3-pro".into(),
            display_name: "o3 pro".into(),
            capabilities: caps_o1(200_000, 100_000, true, true),
        },
        ModelInfo {
            id: "o3-deep-research".into(),
            native_id: "o3-deep-research".into(),
            display_name: "o3 deep research".into(),
            capabilities: caps_o1(200_000, 100_000, true, true),
        },
        ModelInfo {
            id: "o4-mini".into(),
            native_id: "o4-mini".into(),
            display_name: "o4 mini".into(),
            capabilities: caps_o1(200_000, 100_000, true, true),
        },
        ModelInfo {
            id: "o4-mini-deep-research".into(),
            native_id: "o4-mini-deep-research".into(),
            display_name: "o4 mini deep research".into(),
            capabilities: caps_o1(200_000, 100_000, true, true),
        },
        // GPT-4.1 — legacy
        ModelInfo {
            id: "gpt-4.1".into(),
            native_id: "gpt-4.1".into(),
            display_name: "GPT-4.1".into(),
            capabilities: caps(1_000_000, 32_768, true, false),
        },
        ModelInfo {
            id: "gpt-4.1-mini".into(),
            native_id: "gpt-4.1-mini".into(),
            display_name: "GPT-4.1 mini".into(),
            capabilities: caps(1_000_000, 32_768, true, false),
        },
        ModelInfo {
            id: "gpt-4.1-nano".into(),
            native_id: "gpt-4.1-nano".into(),
            display_name: "GPT-4.1 nano".into(),
            capabilities: caps(1_000_000, 32_768, true, false),
        },
        // GPT-4o — legacy
        ModelInfo {
            id: "gpt-4o".into(),
            native_id: "gpt-4o".into(),
            display_name: "GPT-4o (legacy)".into(),
            capabilities: caps(128_000, 4_096, true, false),
        },
        ModelInfo {
            id: "gpt-4o-mini".into(),
            native_id: "gpt-4o-mini".into(),
            display_name: "GPT-4o mini (legacy)".into(),
            capabilities: caps(128_000, 16_384, true, false),
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

/// Whether the given model is in a family that requires `max_completion_tokens`
/// instead of `max_tokens`.
///
/// `OpenAI`'s GPT-5 family and the o-series of reasoning models (`o1*`, `o3*`,
/// `o4*`) reject `max_tokens` with HTTP 400 and the explicit error
/// `"Unsupported parameter: 'max_tokens' is not supported with this model.
/// Use 'max_completion_tokens' instead."`
///
/// Matching is case-insensitive prefix on the canonical model ID.
#[must_use]
pub fn uses_completion_tokens(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("gpt-5") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

#[cfg(test)]
mod tests {
    use super::uses_completion_tokens;

    #[test]
    fn gpt5_family_uses_completion_tokens() {
        assert!(uses_completion_tokens("gpt-5"));
        assert!(uses_completion_tokens("gpt-5-mini"));
        assert!(uses_completion_tokens("gpt-5-nano"));
    }

    #[test]
    fn o_series_uses_completion_tokens() {
        assert!(uses_completion_tokens("o1"));
        assert!(uses_completion_tokens("o1-mini"));
        assert!(uses_completion_tokens("o1-preview"));
        assert!(uses_completion_tokens("o3"));
        assert!(uses_completion_tokens("o3-mini"));
        assert!(uses_completion_tokens("o4"));
        assert!(uses_completion_tokens("o4-mini"));
    }

    #[test]
    fn non_reasoning_models_use_max_tokens() {
        assert!(!uses_completion_tokens("gpt-4o"));
        assert!(!uses_completion_tokens("gpt-4o-mini"));
        assert!(!uses_completion_tokens("gpt-4.1"));
        assert!(!uses_completion_tokens("gpt-3.5-turbo"));
        assert!(!uses_completion_tokens("qwen3.5-9b-mlx"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(uses_completion_tokens("GPT-5"));
        assert!(uses_completion_tokens("O1"));
        assert!(uses_completion_tokens("O3-MINI"));
        assert!(uses_completion_tokens("O4-Mini"));
    }
}
