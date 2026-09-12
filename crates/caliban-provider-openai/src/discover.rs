//! Model discovery from an `OpenAI`-compatible server's `/v1/models` response.
//!
//! Recovers the model list and (where the server exposes it) the loaded context
//! window that the removed Ollama provider used to detect — but through the
//! `OpenAI` adapter, so it works for any `OpenAI`-compatible local engine (ADR 0056).
//!
//! Observed shapes (`scripts/probe-discovery-endpoints.sh`, 2026-09-12):
//! - **llama.cpp** `/v1/models` `.data[]` carries a rich `meta`, including
//!   `n_ctx` (the *loaded* context window) and `n_ctx_train` (the model max).
//!   This makes a separate `/props` call unnecessary for context detection.
//! - **mlx-lm** `/v1/models` `.data[]` is `{id, object, created}` only — no
//!   context or capability info, so it degrades to the static baseline.
//! - **llama-swap** aggregate `/v1/models` lists the configured model ids with a
//!   `status` but no `meta` — also the static baseline.

use caliban_provider::{Capabilities, ModelInfo};

/// Parse a `/v1/models` JSON body into `ModelInfo` entries.
///
/// Each `data[]` entry's `id` seeds a `ModelInfo`; its capabilities start from
/// the static per-id table ([`crate::models::capabilities_for`]) and, when the
/// entry carries llama.cpp's `meta.n_ctx`, `max_input_tokens` is overridden with
/// that loaded context window. Entries without an `id`, and a body without a
/// `data` array, are skipped — callers fall back to the static table.
pub(crate) fn parse_models(json: &serde_json::Value) -> Vec<ModelInfo> {
    let Some(data) = json.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|entry| {
            let id = entry
                .get("id")
                .and_then(serde_json::Value::as_str)?
                .to_string();
            let mut caps: Capabilities = crate::models::capabilities_for(&id);
            if let Some(n) = loaded_context_window(entry) {
                caps.max_input_tokens = n;
            }
            Some(ModelInfo {
                id: id.clone(),
                native_id: id.clone(),
                display_name: id,
                capabilities: caps,
            })
        })
        .collect()
}

/// Extract llama.cpp's loaded context window (`meta.n_ctx`) from a `/v1/models`
/// entry, if present and representable. Returns `None` for engines that don't
/// expose it (mlx-lm, the llama-swap aggregate list).
fn loaded_context_window(entry: &serde_json::Value) -> Option<u32> {
    let n_ctx = entry
        .get("meta")
        .and_then(|m| m.get("n_ctx"))
        .and_then(serde_json::Value::as_u64)?;
    u32::try_from(n_ctx).ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::parse_models;
    use crate::models::capabilities_for;

    // Real llama.cpp `/v1/models` shape (probe output): rich meta with n_ctx.
    #[test]
    fn llamacpp_meta_n_ctx_overrides_max_input_tokens() {
        let json = serde_json::json!({
            "object": "list",
            "data": [{
                "id": "unsloth/Qwen3.6-27B-GGUF:Q4_K_M",
                "owned_by": "llamacpp",
                "meta": { "n_ctx": 8192, "n_ctx_train": 262_144, "n_params": 26_895_998_464_u64 }
            }]
        });
        let models = parse_models(&json);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "unsloth/Qwen3.6-27B-GGUF:Q4_K_M");
        assert_eq!(models[0].native_id, "unsloth/Qwen3.6-27B-GGUF:Q4_K_M");
        assert_eq!(
            models[0].capabilities.max_input_tokens, 8192,
            "loaded n_ctx should override max_input_tokens"
        );
    }

    // Real mlx-lm `/v1/models` shape: id only, no meta -> static baseline caps.
    #[test]
    fn mlx_minimal_entry_falls_back_to_static_caps() {
        let json = serde_json::json!({
            "object": "list",
            "data": [{ "id": "mlx-community/Qwen3.6-27B-4bit", "object": "model", "created": 1 }]
        });
        let models = parse_models(&json);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "mlx-community/Qwen3.6-27B-4bit");
        assert_eq!(
            models[0].capabilities.max_input_tokens,
            capabilities_for("mlx-community/Qwen3.6-27B-4bit").max_input_tokens,
            "no meta -> unchanged from the static baseline"
        );
    }

    // Real llama-swap aggregate shape: ids + status, no meta -> all listed.
    #[test]
    fn llama_swap_aggregate_lists_all_ids() {
        let json = serde_json::json!({
            "object": "list",
            "data": [
                { "id": "mlx-community/Qwen3.6-27B-4bit", "status": { "value": "unloaded" } },
                { "id": "qwen3.6-27b-gguf", "status": { "value": "unloaded" } }
            ]
        });
        let ids: Vec<String> = parse_models(&json).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["mlx-community/Qwen3.6-27B-4bit", "qwen3.6-27b-gguf"]);
    }

    #[test]
    fn missing_or_empty_data_yields_empty() {
        assert!(parse_models(&serde_json::json!({})).is_empty());
        assert!(parse_models(&serde_json::json!({ "data": [] })).is_empty());
        // entries without an id are skipped
        assert!(parse_models(&serde_json::json!({ "data": [{ "object": "model" }] })).is_empty());
    }
}
