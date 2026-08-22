//! The vendored `OpenRouter` model catalogue.
//!
//! `OpenRouter` fronts hundreds of models whose capabilities differ wildly, so a
//! hardcoded table (the shape [`caliban_provider_openai::models`] uses) cannot
//! describe them. The catalogue is therefore **vendored** from
//! `GET /api/v1/models` by `scripts/refresh-openrouter-models.sh` and embedded
//! at compile time — the same posture as the telemetry rate card (ADR 0033),
//! and for the same reason: a third-party endpoint must never sit on caliban's
//! boot path (#573).
//!
//! Operators can point at a newer snapshot with `CALIBAN_OPENROUTER_MODELS`.
//! The live path is [`crate::OpenRouterProvider::refresh_models`], which is
//! opt-in by contract.

use std::collections::HashMap;

use caliban_provider::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};
use serde::Deserialize;

/// Failure to load or parse a catalogue snapshot.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// The snapshot named by `CALIBAN_OPENROUTER_MODELS` could not be read.
    #[error("reading OpenRouter catalogue from {path}: {source}")]
    Read {
        /// The path that could not be read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The snapshot was not valid JSON in the expected shape.
    #[error("parsing OpenRouter catalogue{}: {source}", .path.as_deref().map(|p| format!(" from {p}")).unwrap_or_default())]
    Parse {
        /// The path, when the snapshot came from disk rather than the binary.
        path: Option<String>,
        /// The underlying deserialization error.
        #[source]
        source: serde_json::Error,
    },
    /// The snapshot parsed but declares a schema this build does not understand.
    #[error("OpenRouter catalogue schema_version {found} is not supported (expected {expected})")]
    SchemaVersion {
        /// The version found in the snapshot.
        found: u32,
        /// The version this build understands.
        expected: u32,
    },
}

/// Schema version this build understands. Bump in lockstep with the `jq`
/// projection in `scripts/refresh-openrouter-models.sh`.
pub const SCHEMA_VERSION: u32 = 1;

/// Env var naming an alternative snapshot on disk.
pub const SNAPSHOT_ENV: &str = "CALIBAN_OPENROUTER_MODELS";

const EMBEDDED: &str = include_str!("../models.json");

/// One model as recorded in the snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    /// `OpenRouter` model id, in `vendor/model` form.
    pub id: String,
    /// Total context window in tokens.
    pub context_length: u32,
    /// Maximum completion tokens, when the upstream provider reports one.
    pub max_completion_tokens: Option<u32>,
    /// Accepted input modalities (`text`, `image`, `audio`, ...).
    #[serde(default)]
    pub input_modalities: Vec<String>,
    /// Request parameters the model accepts (`tools`, `reasoning`, ...).
    #[serde(default)]
    pub supported_parameters: Vec<String>,
    /// Per-token USD pricing, as reported by `OpenRouter`.
    #[serde(default)]
    pub pricing: Pricing,
}

/// Per-token USD pricing. `OpenRouter` reports these as decimal strings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Pricing {
    /// USD per input token.
    pub prompt: Option<String>,
    /// USD per output token.
    pub completion: Option<String>,
}

impl Pricing {
    /// Input price in USD per million tokens, when parseable.
    #[must_use]
    pub fn input_per_mtok(&self) -> Option<f64> {
        self.prompt.as_deref()?.parse::<f64>().ok().map(|v| v * 1e6)
    }

    /// Output price in USD per million tokens, when parseable.
    #[must_use]
    pub fn output_per_mtok(&self) -> Option<f64> {
        self.completion
            .as_deref()?
            .parse::<f64>()
            .ok()
            .map(|v| v * 1e6)
    }
}

#[derive(Debug, Deserialize)]
struct Snapshot {
    schema_version: u32,
    #[serde(default)]
    models: Vec<ModelEntry>,
}

/// An indexed `OpenRouter` catalogue.
#[derive(Debug, Clone)]
pub struct Catalog {
    by_id: HashMap<String, ModelEntry>,
}

impl Catalog {
    /// Load the snapshot embedded at compile time.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] if the embedded snapshot fails to parse or
    /// declares an unsupported schema version — both are build defects.
    pub fn embedded() -> Result<Self, CatalogError> {
        Self::from_str_inner(EMBEDDED, None)
    }

    /// Load from `CALIBAN_OPENROUTER_MODELS` when set, else the embedded snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] if the named file cannot be read, or if either
    /// snapshot fails to parse. An operator who points at a bad file gets a
    /// hard error rather than a silent fall back to stale data.
    pub fn load() -> Result<Self, CatalogError> {
        match std::env::var(SNAPSHOT_ENV) {
            Ok(path) if !path.is_empty() => Self::from_path(std::path::Path::new(&path)),
            _ => Self::embedded(),
        }
    }

    fn from_str_inner(raw: &str, path: Option<&str>) -> Result<Self, CatalogError> {
        let snap: Snapshot = serde_json::from_str(raw).map_err(|source| CatalogError::Parse {
            path: path.map(ToOwned::to_owned),
            source,
        })?;
        if snap.schema_version != SCHEMA_VERSION {
            return Err(CatalogError::SchemaVersion {
                found: snap.schema_version,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(Self {
            by_id: snap.models.into_iter().map(|m| (m.id.clone(), m)).collect(),
        })
    }

    /// Build a catalogue from a live `GET /api/v1/models` body.
    ///
    /// The live payload nests the fields the vendored snapshot flattens, so
    /// this deserializes the upstream shape directly rather than reusing
    /// [`Snapshot`]. Keep it in step with the `jq` projection in
    /// `scripts/refresh-openrouter-models.sh`.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::Parse`] if the body is not the expected shape.
    pub fn from_live_json(raw: &str) -> Result<Self, CatalogError> {
        #[derive(Deserialize)]
        struct Live {
            data: Vec<LiveModel>,
        }
        #[derive(Deserialize)]
        struct LiveModel {
            id: String,
            context_length: u32,
            #[serde(default)]
            top_provider: TopProvider,
            #[serde(default)]
            architecture: Architecture,
            #[serde(default)]
            supported_parameters: Vec<String>,
            #[serde(default)]
            pricing: Pricing,
        }
        #[derive(Default, Deserialize)]
        struct TopProvider {
            #[serde(default)]
            max_completion_tokens: Option<u32>,
        }
        #[derive(Default, Deserialize)]
        struct Architecture {
            #[serde(default)]
            input_modalities: Vec<String>,
        }

        let live: Live = serde_json::from_str(raw)
            .map_err(|source| CatalogError::Parse { path: None, source })?;
        Ok(Self {
            by_id: live
                .data
                .into_iter()
                .map(|m| {
                    (
                        m.id.clone(),
                        ModelEntry {
                            id: m.id,
                            context_length: m.context_length,
                            max_completion_tokens: m.top_provider.max_completion_tokens,
                            input_modalities: m.architecture.input_modalities,
                            supported_parameters: m.supported_parameters,
                            pricing: m.pricing,
                        },
                    )
                })
                .collect(),
        })
    }

    /// Parse a snapshot from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] if the JSON is malformed or declares an
    /// unsupported `schema_version`.
    pub fn from_snapshot_json(raw: &str) -> Result<Self, CatalogError> {
        Self::from_str_inner(raw, None)
    }

    /// Load a snapshot from an explicit path.
    ///
    /// This is the seam [`Self::load`] uses for the `CALIBAN_OPENROUTER_MODELS`
    /// override, exposed so tests can exercise it without mutating process
    /// environment (which the workspace's `unsafe_code` deny would forbid).
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] if the file cannot be read or parsed.
    pub fn from_path(path: &std::path::Path) -> Result<Self, CatalogError> {
        let display = path.display().to_string();
        let raw = std::fs::read_to_string(path).map_err(|source| CatalogError::Read {
            path: display.clone(),
            source,
        })?;
        Self::from_str_inner(&raw, Some(&display))
    }

    /// Number of models in the catalogue.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the catalogue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Look up one model.
    #[must_use]
    pub fn get(&self, model: &str) -> Option<&ModelEntry> {
        self.by_id.get(model)
    }

    /// Whether the catalogue describes `model`.
    ///
    /// Callers that can fail loudly (config validation, `caliban router debug`)
    /// should use this rather than inferring from [`Self::capabilities_for`],
    /// whose `None` is deliberately non-fatal at request time.
    #[must_use]
    pub fn contains(&self, model: &str) -> bool {
        self.by_id.contains_key(model)
    }

    /// Capabilities for `model`, or `None` when the catalogue does not describe it.
    ///
    /// Returning `None` rather than a blanket default is the point of this
    /// crate: the `OpenAI` adapter's `capabilities_for` silently answers
    /// `caps(128_000, 4_096, false, false)` for every unknown id, which
    /// mis-describes every `OpenRouter` model at once (#573).
    #[must_use]
    pub fn capabilities_for(&self, model: &str) -> Option<Capabilities> {
        self.by_id.get(model).map(capabilities_of)
    }

    /// Every model in the catalogue, as [`ModelInfo`].
    #[must_use]
    pub fn model_infos(&self) -> Vec<ModelInfo> {
        let mut out: Vec<ModelInfo> = self
            .by_id
            .values()
            .map(|m| ModelInfo {
                id: m.id.clone(),
                native_id: m.id.clone(),
                display_name: m.id.clone(),
                capabilities: capabilities_of(m),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

/// Map one catalogue entry onto caliban's [`Capabilities`].
///
/// Every field is derived from data `OpenRouter` actually publishes, or left at a
/// conservative value with a comment saying why. Nothing here guesses upward.
#[must_use]
pub fn capabilities_of(m: &ModelEntry) -> Capabilities {
    let has = |p: &str| m.supported_parameters.iter().any(|s| s == p);

    Capabilities {
        max_input_tokens: m.context_length,
        // OpenRouter reports `max_completion_tokens` only when the upstream
        // provider does. When absent the context window is the real ceiling —
        // still far better than the 4096 the OpenAI fallback would assert.
        max_output_tokens: m.max_completion_tokens.unwrap_or(m.context_length),
        vision: m.input_modalities.iter().any(|s| s == "image"),
        // `supported_parameters` distinguishes tools/no-tools but not
        // single-vs-parallel. OpenRouter's chat surface is OpenAI-shaped, whose
        // `tool_calls` is an array, so parallel is the correct reading.
        tool_use: if has("tools") || has("tool_choice") {
            ToolUseCapability::ParallelCalls
        } else {
            ToolUseCapability::None
        },
        thinking: has("reasoning") || has("reasoning_effort") || has("include_reasoning"),
        // Deliberately conservative: caching on OpenRouter depends on the
        // upstream provider and is not advertised per-model. Claiming
        // `Automatic` here (as the OpenAI fallback does) would be a guess.
        prompt_caching: PromptCachingCapability::None,
        json_mode: has("response_format") || has("structured_outputs"),
        // OpenRouter streams every model over its OpenAI-compatible surface.
        streaming: true,
        stop_sequences: has("stop"),
        top_k: has("top_k"),
        // OpenAI-compatible chat completions take the system prompt as a message.
        system_prompt: SystemPromptCapability::SystemRole,
        // `refusal` is an OpenAI-specific response field; not guaranteed here.
        refusal_field: false,
    }
}
