//! Runtime model discovery for Ollama (#316).
//!
//! The Ollama server is the source of truth for which models exist and what
//! they can do. This module maps the server's `/api/show` metadata onto
//! caliban's [`Capabilities`], so the static `models` table is a fallback of
//! last resort rather than the authority (static data drifts and is usually
//! wrong for a provider whose model set is dynamic).

use std::path::{Path, PathBuf};

use caliban_provider::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};
use serde::{Deserialize, Serialize};

use crate::schema::ModelShow;

/// Conservative context window used only when the server reports no
/// `context_length` (older Ollama, or a model that omits it). Intentionally
/// small: under-promising truncates but never exceeds the model's real limit,
/// whereas an optimistic guess can overflow it.
pub const BOOTSTRAP_CONTEXT: u32 = 8_192;

/// Default output-token cap. Ollama does not report a max-output; this mirrors
/// the previous static-table value.
pub const DEFAULT_MAX_OUTPUT: u32 = 8_192;

/// Honest default for a model we have no discovery data for (server
/// unreachable + no cache). Permissive on capabilities so the router doesn't
/// refuse to route, with the conservative [`BOOTSTRAP_CONTEXT`] window — never
/// the old, frequently-wrong static 32K.
#[must_use]
pub fn bootstrap_capabilities() -> Capabilities {
    Capabilities {
        max_input_tokens: BOOTSTRAP_CONTEXT,
        max_output_tokens: DEFAULT_MAX_OUTPUT,
        vision: false,
        // Optimistic: an unknown model might support tools; a real call fails
        // loudly rather than the router silently filtering it out.
        tool_use: ToolUseCapability::Basic,
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

/// Build [`Capabilities`] from a model's `/api/show` metadata.
///
/// Context window comes from `model_info[*.context_length]` (the caller may
/// later overlay the live `/api/ps` value). The boolean/enum capabilities are
/// read from the server's `capabilities` array
/// (`completion`/`vision`/`tools`/`thinking`) — no static guessing.
#[must_use]
pub fn capabilities_from_show(show: &ModelShow) -> Capabilities {
    let has = |c: &str| show.capabilities.iter().any(|x| x == c);
    Capabilities {
        max_input_tokens: show.context_length().unwrap_or(BOOTSTRAP_CONTEXT),
        max_output_tokens: DEFAULT_MAX_OUTPUT,
        vision: has("vision"),
        tool_use: if has("tools") {
            ToolUseCapability::ParallelCalls
        } else {
            ToolUseCapability::None
        },
        thinking: has("thinking"),
        prompt_caching: PromptCachingCapability::None,
        json_mode: true,
        streaming: true,
        stop_sequences: true,
        top_k: true,
        system_prompt: SystemPromptCapability::SystemRole,
        refusal_field: false,
    }
}

// ---------------------------------------------------------------------------
// Persisted discovery cache
// ---------------------------------------------------------------------------

/// On-disk envelope for the last successful discovery, so a cold start (or a
/// briefly-unreachable server) has correct last-known-good values instead of a
/// wrong static default.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    /// The discovered models. Serialized via `ModelInfo`'s derives.
    models: Vec<ModelInfo>,
}

/// Resolve the cache path for a server id: `$XDG_CACHE_HOME/caliban/discovery/
/// ollama-<server>.json` (ADR 0050). `None` if no cache dir is resolvable.
#[must_use]
pub fn default_cache_path(server_id: &str) -> Option<PathBuf> {
    let sanitized: String = server_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    caliban_common::paths::platform_cache_dir().map(|d| {
        d.join("caliban")
            .join("discovery")
            .join(format!("ollama-{sanitized}.json"))
    })
}

/// Load the persisted discovery result. Any error (missing file, malformed
/// JSON) yields an empty list — the caller then relies on a live refresh.
#[must_use]
pub fn load_cache(path: &Path) -> Vec<ModelInfo> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<CacheFile>(&raw)
        .map(|c| c.models)
        .unwrap_or_default()
}

/// Persist a discovery result, creating parent directories as needed. Errors
/// are swallowed (a failed cache write must never fail a refresh).
pub fn save_cache(path: &Path, models: &[ModelInfo]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&CacheFile {
        models: models.to_vec(),
    }) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn show(caps: &[&str], ctx: Option<u32>) -> ModelShow {
        let mut model_info = HashMap::new();
        if let Some(c) = ctx {
            model_info.insert("general.architecture".to_string(), "qwen3_5".into());
            model_info.insert(
                "qwen3_5.context_length".to_string(),
                serde_json::Value::from(c),
            );
        }
        ModelShow {
            model_info,
            capabilities: caps.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn maps_full_capability_set() {
        let caps = capabilities_from_show(&show(
            &["completion", "vision", "thinking", "tools"],
            Some(262_144),
        ));
        assert_eq!(caps.max_input_tokens, 262_144);
        assert!(caps.vision);
        assert!(caps.thinking);
        assert_eq!(caps.tool_use, ToolUseCapability::ParallelCalls);
    }

    #[test]
    fn maps_completion_only_as_no_extras() {
        let caps = capabilities_from_show(&show(&["completion"], Some(8_192)));
        assert!(!caps.vision);
        assert!(!caps.thinking);
        assert_eq!(caps.tool_use, ToolUseCapability::None);
        assert!(caps.streaming, "completion implies streaming");
    }

    #[test]
    fn missing_context_uses_conservative_bootstrap() {
        let caps = capabilities_from_show(&show(&["completion"], None));
        assert_eq!(caps.max_input_tokens, BOOTSTRAP_CONTEXT);
    }

    fn model(id: &str, ctx: u32) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            native_id: id.to_string(),
            display_name: id.to_string(),
            capabilities: capabilities_from_show(&show(&["completion", "tools"], Some(ctx))),
        }
    }

    #[test]
    fn cache_round_trips_models() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sub").join("ollama-x.json");
        let models = vec![model("qwen3.6:27b-mlx", 262_144)];
        save_cache(&path, &models);
        assert!(path.exists(), "save_cache creates the file + parents");
        let loaded = load_cache(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "qwen3.6:27b-mlx");
        assert_eq!(loaded[0].capabilities.max_input_tokens, 262_144);
        assert_eq!(
            loaded[0].capabilities.tool_use,
            ToolUseCapability::ParallelCalls
        );
    }

    #[test]
    fn load_cache_missing_or_garbage_is_empty() {
        assert!(load_cache(Path::new("/nonexistent/x.json")).is_empty());
        let tmp = tempfile::TempDir::new().unwrap();
        let bad = tmp.path().join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        assert!(load_cache(&bad).is_empty());
    }

    #[test]
    fn cache_path_is_under_discovery_dir() {
        // XDG_CACHE_HOME is honored by platform_cache_dir; assert the shape.
        if let Some(p) = default_cache_path("192.168.1.240:11434") {
            let s = p.to_string_lossy();
            assert!(s.contains("caliban/discovery/ollama-"), "got {s}");
            assert!(s.ends_with(".json"));
            // host sanitized: no ':' or '.' in the filename segment.
            assert!(!p.file_name().unwrap().to_string_lossy().contains(':'));
        }
    }
}
