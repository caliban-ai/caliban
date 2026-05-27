//! Static `ModelInfo` table for Ollama models.
//!
//! Ollama identifiers are conventionally `<family>:<tag>`, where the tag
//! distinguishes parameter count or quantisation (e.g. `qwen3.5:9b`,
//! `llama3.2:3b-instruct-q4_K_M`). [`capabilities_for`] does a two-step
//! lookup: first an exact match against the table, then a base-family
//! match after stripping the `:<tag>` suffix. Anything still unmatched
//! falls through to a permissive default that assumes a modern,
//! tool-capable Ollama install — operators who hand-typed an unfamiliar
//! model name almost always want it usable rather than silently filtered
//! out by the router's capability check.

use caliban_provider::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};

const fn caps(
    max_input: u32,
    max_output: u32,
    vision: bool,
    tool_use: bool,
    thinking: bool,
) -> Capabilities {
    Capabilities {
        max_input_tokens: max_input,
        max_output_tokens: max_output,
        vision,
        tool_use: if tool_use {
            ToolUseCapability::Basic
        } else {
            ToolUseCapability::None
        },
        thinking,
        prompt_caching: PromptCachingCapability::None,
        json_mode: true,
        streaming: true,
        stop_sequences: true,
        top_k: true,
        system_prompt: SystemPromptCapability::SystemRole,
        refusal_field: false,
    }
}

/// Capability profile for a model family whose specific tag is unknown.
///
/// Permissive on purpose: an unknown model could be anything from a tiny
/// non-tool-capable Phi up to a 70B reasoning Qwen. We optimistically
/// claim tool support so the router doesn't refuse to route to it; if
/// the actual model can't handle tools, the upstream call will fail
/// loudly rather than silently being filtered out at config time.
const FALLBACK: Capabilities = caps(32_768, 8_192, false, true, false);

/// Return the full list of known Ollama model families.
///
/// IDs are *family* names (no `:<tag>` suffix); the lookup helper does
/// the suffix-strip so `llama3.2:3b` matches the `llama3.2` entry.
#[must_use]
pub fn models() -> Vec<ModelInfo> {
    vec![
        // ----- Llama -----
        ModelInfo {
            id: "llama3.1".into(),
            native_id: "llama3.1".into(),
            display_name: "Llama 3.1 (Ollama)".into(),
            capabilities: caps(128_000, 8_192, false, true, false),
        },
        ModelInfo {
            id: "llama3.2".into(),
            native_id: "llama3.2".into(),
            display_name: "Llama 3.2 (Ollama)".into(),
            capabilities: caps(128_000, 8_192, false, true, false),
        },
        ModelInfo {
            id: "llama3.2-vision".into(),
            native_id: "llama3.2-vision".into(),
            display_name: "Llama 3.2 Vision (Ollama)".into(),
            capabilities: caps(128_000, 8_192, true, true, false),
        },
        ModelInfo {
            id: "llama3.3".into(),
            native_id: "llama3.3".into(),
            display_name: "Llama 3.3 (Ollama)".into(),
            capabilities: caps(128_000, 8_192, false, true, false),
        },
        // ----- Qwen -----
        ModelInfo {
            id: "qwen2.5".into(),
            native_id: "qwen2.5".into(),
            display_name: "Qwen 2.5 (Ollama)".into(),
            capabilities: caps(32_768, 8_192, false, true, false),
        },
        ModelInfo {
            id: "qwen2.5-coder".into(),
            native_id: "qwen2.5-coder".into(),
            display_name: "Qwen 2.5 Coder (Ollama)".into(),
            capabilities: caps(32_768, 8_192, false, true, false),
        },
        ModelInfo {
            id: "qwen3".into(),
            native_id: "qwen3".into(),
            display_name: "Qwen 3 (Ollama, reasoning)".into(),
            capabilities: caps(32_768, 8_192, false, true, true),
        },
        ModelInfo {
            id: "qwen3.5".into(),
            native_id: "qwen3.5".into(),
            display_name: "Qwen 3.5 (Ollama, reasoning)".into(),
            capabilities: caps(32_768, 8_192, false, true, true),
        },
        // ----- DeepSeek reasoning -----
        ModelInfo {
            id: "deepseek-r1".into(),
            native_id: "deepseek-r1".into(),
            display_name: "DeepSeek R1 (Ollama, reasoning)".into(),
            capabilities: caps(64_000, 8_192, false, true, true),
        },
        // ----- Mistral -----
        ModelInfo {
            id: "mistral".into(),
            native_id: "mistral".into(),
            display_name: "Mistral (Ollama)".into(),
            capabilities: caps(32_768, 8_192, false, true, false),
        },
        ModelInfo {
            id: "mistral-nemo".into(),
            native_id: "mistral-nemo".into(),
            display_name: "Mistral Nemo (Ollama)".into(),
            capabilities: caps(128_000, 8_192, false, true, false),
        },
        ModelInfo {
            id: "mistral-small".into(),
            native_id: "mistral-small".into(),
            display_name: "Mistral Small (Ollama)".into(),
            capabilities: caps(32_768, 8_192, false, true, false),
        },
        // ----- Gemma -----
        ModelInfo {
            id: "gemma2".into(),
            native_id: "gemma2".into(),
            display_name: "Gemma 2 (Ollama)".into(),
            capabilities: caps(8_192, 4_096, false, false, false),
        },
        ModelInfo {
            id: "gemma3".into(),
            native_id: "gemma3".into(),
            display_name: "Gemma 3 (Ollama)".into(),
            capabilities: caps(128_000, 8_192, true, false, false),
        },
        // ----- Phi -----
        ModelInfo {
            id: "phi3".into(),
            native_id: "phi3".into(),
            display_name: "Phi 3 (Ollama)".into(),
            capabilities: caps(4_096, 4_096, false, false, false),
        },
        ModelInfo {
            id: "phi4".into(),
            native_id: "phi4".into(),
            display_name: "Phi 4 (Ollama)".into(),
            capabilities: caps(16_000, 8_192, false, true, false),
        },
    ]
}

/// Look up `Capabilities` for a model by canonical or native ID.
///
/// Performs an exact-match lookup first, then strips any `:<tag>`
/// suffix to match by base family (so `qwen3.5:9b` matches the
/// `qwen3.5` entry). Falls back to a permissive default (see
/// [`FALLBACK`]) for anything still unmatched.
#[must_use]
pub fn capabilities_for(model: &str) -> Capabilities {
    let all = models();

    // Step 1: exact match (covers the tagless case + any future tagged entries).
    if let Some(m) = all.iter().find(|m| m.id == model || m.native_id == model) {
        return m.capabilities;
    }

    // Step 2: strip the `:<tag>` suffix and look up the base family.
    if let Some((base, _tag)) = model.split_once(':')
        && let Some(m) = all.iter().find(|m| m.id == base || m.native_id == base)
    {
        return m.capabilities;
    }

    FALLBACK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_returns_registered_caps() {
        let c = capabilities_for("qwen2.5");
        assert_eq!(c.max_input_tokens, 32_768);
        assert!(matches!(c.tool_use, ToolUseCapability::Basic));
        assert!(!c.thinking);
    }

    #[test]
    fn tagged_variant_strips_to_base_family() {
        // The probe instance serves qwen3.5:9b; the static table only
        // knows the base family `qwen3.5`. The lookup must still return
        // the reasoning-capable caps so the router doesn't refuse it.
        let c = capabilities_for("qwen3.5:9b");
        assert!(matches!(c.tool_use, ToolUseCapability::Basic));
        assert!(c.thinking, "qwen3.5 is a reasoning model");
    }

    #[test]
    fn deeply_tagged_variant_strips_to_base_family() {
        // Real Ollama tags include quant suffixes too: `:8b-instruct-q4_K_M`.
        // We only split on the first `:`, so the base lookup still works.
        let c = capabilities_for("llama3.2:3b-instruct-q4_K_M");
        assert!(matches!(c.tool_use, ToolUseCapability::Basic));
        assert_eq!(c.max_input_tokens, 128_000);
    }

    #[test]
    fn unknown_model_falls_back_permissively() {
        // Regression: the previous fallback was `tool_use: None`, which
        // made the router refuse every unlisted model. Operators who
        // pulled a fresh Ollama tag should not be silently filtered out.
        let c = capabilities_for("some-future-fancy-model:42b");
        assert!(matches!(c.tool_use, ToolUseCapability::Basic));
        assert!(c.max_input_tokens >= 32_768);
    }

    #[test]
    fn known_reasoning_models_advertise_thinking() {
        assert!(capabilities_for("qwen3").thinking);
        assert!(capabilities_for("qwen3.5").thinking);
        assert!(capabilities_for("deepseek-r1").thinking);
        // Non-reasoning models must NOT claim thinking just because the
        // permissive fallback is in play.
        assert!(!capabilities_for("llama3.1").thinking);
        assert!(!capabilities_for("mistral").thinking);
    }
}
