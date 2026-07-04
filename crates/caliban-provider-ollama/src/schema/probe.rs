//! Wire-format types for the Ollama capability-probe endpoints.
//!
//! These back the runtime context-window detection (issue #60): the static
//! capability table in [`crate::models`] guesses a model's context length and
//! falls back to 32K, which is wrong by up to 8× for custom/MLX builds. The
//! server knows the real value and exposes it two ways:
//!
//! - `GET /api/ps` — [`RunningModelList`] — `context_length` per *currently
//!   loaded* model (the live value, honoring the runtime `num_ctx`).
//! - `POST /api/show` — [`ModelShow`] — `model_info["<arch>.context_length"]`
//!   (the model's maximum, available even when the model is not loaded).

use std::collections::HashMap;

use serde::Deserialize;

/// Response body of `GET /api/ps` (the loaded-model list).
#[derive(Debug, Clone, Deserialize)]
pub struct RunningModelList {
    /// Currently-loaded models. Empty when nothing is resident.
    #[serde(default)]
    pub models: Vec<RunningModel>,
}

/// A single entry in the `GET /api/ps` model list.
#[derive(Debug, Clone, Deserialize)]
pub struct RunningModel {
    /// The model's user-facing name (e.g. `qwen3.6:27b-mlx`).
    #[serde(default)]
    pub name: String,
    /// The model identifier (usually identical to `name`).
    #[serde(default)]
    pub model: String,
    /// The live context length the model is loaded with. Absent on older
    /// Ollama builds that predate the field.
    #[serde(default)]
    pub context_length: Option<u32>,
}

impl RunningModel {
    /// Whether this loaded model corresponds to the given wire id. Ollama
    /// populates both `name` and `model`; either may carry the id.
    #[must_use]
    pub fn matches(&self, wire: &str) -> bool {
        self.name == wire || self.model == wire
    }
}

/// Response body of `POST /api/show` (model metadata).
#[derive(Debug, Clone, Deserialize)]
pub struct ModelShow {
    /// Flat key/value metadata. Keys are architecture-prefixed, e.g.
    /// `qwen3_5.context_length`, `gemma4.context_length`. The architecture
    /// itself is under `general.architecture`.
    #[serde(default)]
    pub model_info: HashMap<String, serde_json::Value>,
    /// Server-reported capability tags, e.g. `["completion", "vision",
    /// "thinking", "tools"]`. Absent on older Ollama builds (defaults empty).
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Response body of `GET /api/tags` (the available/pulled model list).
#[derive(Debug, Clone, Deserialize)]
pub struct TagList {
    /// Available models. Empty when none are pulled.
    #[serde(default)]
    pub models: Vec<TagEntry>,
}

/// A single entry in the `GET /api/tags` list.
#[derive(Debug, Clone, Deserialize)]
pub struct TagEntry {
    /// The model's user-facing name / wire id (e.g. `qwen3.6:27b-mlx`).
    #[serde(default)]
    pub name: String,
    /// Free-form details (family, format, quantization). Often sparse for
    /// safetensors/MLX builds — `/api/show` is the authoritative source.
    #[serde(default)]
    pub details: TagDetails,
}

/// The `details` sub-object of a `/api/tags` entry.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TagDetails {
    /// Model family (e.g. `gemma3`); empty for MLX/safetensors builds.
    #[serde(default)]
    pub family: String,
    /// Weight format (`gguf`, `safetensors`).
    #[serde(default)]
    pub format: String,
}

impl ModelShow {
    /// Extract the model's maximum context length from `model_info`.
    ///
    /// Prefers the architecture-specific key derived from
    /// `general.architecture` (e.g. `qwen3_5.context_length`); if that is
    /// absent, falls back to any key ending in `.context_length`. Returns
    /// `None` when no such key holds a non-negative integer that fits in a
    /// `u32` — so a missing key, a string value, or an absurd number all
    /// degrade gracefully rather than yielding a bogus capacity.
    #[must_use]
    pub fn context_length(&self) -> Option<u32> {
        let as_u32 = |v: &serde_json::Value| v.as_u64().and_then(|n| u32::try_from(n).ok());

        if let Some(arch) = self
            .model_info
            .get("general.architecture")
            .and_then(serde_json::Value::as_str)
            && let Some(n) = self
                .model_info
                .get(&format!("{arch}.context_length"))
                .and_then(as_u32)
        {
            return Some(n);
        }

        self.model_info
            .iter()
            .find(|(k, _)| k.ends_with(".context_length"))
            .and_then(|(_, v)| as_u32(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real /api/ps body captured from the live server (Ollama 0.30.6).
    const PS_BODY: &str = r#"{
        "models": [
            { "name": "qwen3.6:27b-mlx", "model": "qwen3.6:27b-mlx",
              "context_length": 262144, "details": { "format": "safetensors" } }
        ]
    }"#;

    #[test]
    fn ps_extracts_context_length_and_matches_by_name_and_model() {
        let list: RunningModelList = serde_json::from_str(PS_BODY).unwrap();
        let m = &list.models[0];
        assert_eq!(m.context_length, Some(262_144));
        assert!(m.matches("qwen3.6:27b-mlx"));
        assert!(!m.matches("gemma4:12b-mlx"));
    }

    #[test]
    fn ps_empty_list_when_nothing_loaded() {
        let list: RunningModelList = serde_json::from_str(r#"{"models": []}"#).unwrap();
        assert!(list.models.is_empty());
    }

    #[test]
    fn show_extracts_arch_prefixed_context_length_across_real_architectures() {
        // All four prefixes observed on the live server must resolve.
        for (arch, ctx) in [
            ("qwen3_5", 262_144u32),
            ("gemma4_unified", 131_072),
            ("gemma4", 262_144),
            ("gemma3", 32_768),
        ] {
            let body = format!(
                r#"{{"model_info": {{"general.architecture": "{arch}",
                    "{arch}.context_length": {ctx}, "general.parameter_count": 27000000000}}}}"#
            );
            let show: ModelShow = serde_json::from_str(&body).unwrap();
            assert_eq!(show.context_length(), Some(ctx), "arch {arch}");
        }
    }

    #[test]
    fn show_falls_back_to_any_context_length_key_when_arch_missing() {
        // No general.architecture, but a *.context_length key is present.
        let body = r#"{"model_info": {"mystery.context_length": 16384}}"#;
        let show: ModelShow = serde_json::from_str(body).unwrap();
        assert_eq!(show.context_length(), Some(16_384));
    }

    #[test]
    fn show_returns_none_when_no_context_length_present() {
        let body = r#"{"model_info": {"general.architecture": "llama", "general.name": "x"}}"#;
        let show: ModelShow = serde_json::from_str(body).unwrap();
        assert_eq!(show.context_length(), None);
    }

    #[test]
    fn show_returns_none_for_non_integer_or_oversized_values() {
        // Garbage values must not yield a bogus capacity.
        let body =
            r#"{"model_info": {"x.context_length": "lots", "y.context_length": 99999999999}}"#;
        let show: ModelShow = serde_json::from_str(body).unwrap();
        assert_eq!(show.context_length(), None);
    }

    #[test]
    fn show_missing_model_info_is_empty() {
        let show: ModelShow = serde_json::from_str("{}").unwrap();
        assert_eq!(show.context_length(), None);
    }

    #[test]
    fn show_parses_capabilities_array() {
        // Real /api/show shape (Ollama 0.30.6) for qwen3.6:27b-mlx.
        let body = r#"{"model_info": {"general.architecture": "qwen3_5",
            "qwen3_5.context_length": 262144},
            "capabilities": ["completion", "vision", "thinking", "tools"]}"#;
        let show: ModelShow = serde_json::from_str(body).unwrap();
        assert_eq!(
            show.capabilities,
            vec!["completion", "vision", "thinking", "tools"]
        );
        assert_eq!(show.context_length(), Some(262_144));
    }

    #[test]
    fn show_missing_capabilities_is_empty() {
        let show: ModelShow = serde_json::from_str(r#"{"model_info": {}}"#).unwrap();
        assert!(show.capabilities.is_empty());
    }

    #[test]
    fn tags_parses_available_models() {
        // Real /api/tags shape: MLX safetensors entries have empty family.
        let body = r#"{"models": [
            {"name": "qwen3.6:27b-mlx", "details": {"family": "", "format": "safetensors"}},
            {"name": "gemma3:1b", "details": {"family": "gemma3", "format": "gguf"}}
        ]}"#;
        let list: TagList = serde_json::from_str(body).unwrap();
        assert_eq!(list.models.len(), 2);
        assert_eq!(list.models[0].name, "qwen3.6:27b-mlx");
        assert_eq!(list.models[1].name, "gemma3:1b");
    }

    #[test]
    fn tags_empty_when_no_models() {
        let list: TagList = serde_json::from_str(r#"{"models": []}"#).unwrap();
        assert!(list.models.is_empty());
    }
}
