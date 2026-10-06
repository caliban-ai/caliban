//! Shared `TurnEvent` stream-decoding state.
//!
//! The single-prompt and headless drivers each re-implemented the same two
//! pieces of per-run bookkeeping while consuming a `TurnEvent` stream (#154):
//!
//! - **tool-input accumulation** — buffering the `ToolCallInputDelta` JSON
//!   fragments per `tool_use_id` so the complete input is available at
//!   `ToolCallEnd`, and
//! - **model-mismatch dedup** — warning at most once per `(requested, actual)`
//!   model pair when an OpenAI-compatible server silently substitutes a model
//!   (the LM Studio case, F4).
//!
//! [`StreamDecoder`] owns both so each front end keeps only its rendering. The
//! TUI keeps its own accumulation in the live transcript (its UI model, not
//! this flat buffer) and has no mismatch warning; the attach renderer is
//! stateless — so neither carried the duplicated state this type removes.

use std::collections::{HashMap, HashSet};

/// A buffered tool call: its name plus the input JSON accumulated across
/// `ToolCallInputDelta`s.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolInput {
    /// Tool name, captured at `ToolCallStart`.
    pub(crate) name: String,
    /// The complete (once streaming finishes) input JSON.
    pub(crate) json: String,
}

/// The canonical one-line model-mismatch warning shared by the drivers' text
/// output paths.
pub(crate) fn model_mismatch_text(requested: &str, actual: &str) -> String {
    format!(
        "[caliban] warning: model mismatch \u{2014} requested {requested:?} but provider responded with {actual:?}"
    )
}

/// Per-run decode state shared by the streaming drivers.
#[derive(Debug, Default)]
pub(crate) struct StreamDecoder {
    tool_inputs: HashMap<String, ToolInput>,
    seen_mismatches: HashSet<(String, String)>,
    /// Model IDs the provider confirmed via `/v1/models` (empty when the
    /// provider doesn't list models). A stream `model` that differs from the
    /// requested id is a *substitution* only if the requested id is NOT in this
    /// set; when it IS, the differing name is an alias/backend label (llama-swap,
    /// `LiteLLM`, …), not a silent substitution, so the warning is suppressed (#715).
    known_models: HashSet<String>,
}

impl StreamDecoder {
    /// A fresh decoder for one run.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record the provider's confirmed model IDs (its `/v1/models` listing), used
    /// to suppress false-positive mismatch warnings behind alias proxies (#715).
    #[must_use]
    pub(crate) fn with_known_models(mut self, ids: HashSet<String>) -> Self {
        self.known_models = ids;
        self
    }

    /// Record a tool call's start (its name), keyed by `tool_use_id`.
    pub(crate) fn tool_started(&mut self, tool_use_id: String, name: String) {
        self.tool_inputs.insert(
            tool_use_id,
            ToolInput {
                name,
                json: String::new(),
            },
        );
    }

    /// Append a streamed input-JSON fragment to the buffered tool call,
    /// tolerating a delta that arrives before its `ToolCallStart` (it creates
    /// an empty buffer).
    pub(crate) fn tool_input_delta(&mut self, tool_use_id: String, partial_json: &str) {
        self.tool_inputs
            .entry(tool_use_id)
            .or_default()
            .json
            .push_str(partial_json);
    }

    /// Remove and return the buffered tool call (name + complete input JSON).
    pub(crate) fn take_tool_input(&mut self, tool_use_id: &str) -> Option<ToolInput> {
        self.tool_inputs.remove(tool_use_id)
    }

    /// Drop any buffered tool inputs (e.g. between headless passes), leaving
    /// the run-level model-mismatch dedup intact.
    pub(crate) fn clear_tool_inputs(&mut self) {
        self.tool_inputs.clear();
    }

    /// Record a `(requested, actual)` model pair and return `true` the first
    /// time a genuine mismatch is seen, so the caller can warn exactly once.
    /// Returns `false` when the models match, `actual` is empty, or the pair
    /// was already reported.
    pub(crate) fn note_model_mismatch(&mut self, requested: &str, actual: &str) -> bool {
        !actual.is_empty()
            && actual != requested
            // The provider confirmed `requested` exists (it listed it via
            // `/v1/models`), so a differing stream name is an alias/backend
            // label, not a silent substitution — don't warn (#715).
            && !self.known_models.contains(requested)
            && self
                .seen_mismatches
                .insert((requested.to_string(), actual.to_string()))
    }

    /// The canonical first-seen model-mismatch warning text for `(requested,
    /// actual)`, or `None` when there is nothing new to warn about. Convenience
    /// over [`StreamDecoder::note_model_mismatch`] for drivers with a single
    /// text output path.
    pub(crate) fn model_mismatch_warning(
        &mut self,
        requested: &str,
        actual: &str,
    ) -> Option<String> {
        self.note_model_mismatch(requested, actual)
            .then(|| model_mismatch_text(requested, actual))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_tool_input_across_deltas() {
        let mut d = StreamDecoder::new();
        d.tool_started("t1".into(), "Bash".into());
        d.tool_input_delta("t1".into(), "{\"cmd\":");
        d.tool_input_delta("t1".into(), "\"ls\"}");
        let got = d.take_tool_input("t1").expect("buffered");
        assert_eq!(got.name, "Bash");
        assert_eq!(got.json, "{\"cmd\":\"ls\"}");
        // Taken once → gone.
        assert!(d.take_tool_input("t1").is_none());
    }

    #[test]
    fn input_delta_before_start_creates_buffer() {
        let mut d = StreamDecoder::new();
        d.tool_input_delta("t1".into(), "{}");
        let got = d.take_tool_input("t1").expect("buffered");
        assert_eq!(got.name, "");
        assert_eq!(got.json, "{}");
    }

    #[test]
    fn mismatch_warns_once_per_pair() {
        let mut d = StreamDecoder::new();
        assert!(d.note_model_mismatch("a", "b"));
        assert!(!d.note_model_mismatch("a", "b"), "deduped");
        assert!(d.note_model_mismatch("a", "c"), "new pair warns");
    }

    #[test]
    fn mismatch_ignores_match_and_empty() {
        let mut d = StreamDecoder::new();
        assert!(!d.note_model_mismatch("a", "a"), "exact match");
        assert!(!d.note_model_mismatch("a", ""), "empty actual");
    }

    #[test]
    fn mismatch_suppressed_for_known_alias() {
        // #715: llama-swap lists the alias `qwen3.8-27b-gguf` via /v1/models and
        // proxies to llama.cpp, which reports its own `-hf` name in the stream.
        // The requested id is confirmed present, so the differing stream name is
        // an alias/backend label — no warning.
        let known: HashSet<String> = ["qwen3.8-27b-gguf".to_string()].into_iter().collect();
        let mut d = StreamDecoder::new().with_known_models(known);
        assert!(
            !d.note_model_mismatch("qwen3.8-27b-gguf", "unsloth/Qwen3.8-27B-GGUF:UD-Q5_K_M"),
            "a confirmed id differing from the stream model is an alias, not a substitution"
        );
    }

    #[test]
    fn mismatch_warns_for_genuine_substitution() {
        // A genuine substitution (LM Studio serves a *different loaded model* for
        // an unknown id): the requested id is NOT in /v1/models, so it must still
        // warn exactly once.
        let known: HashSet<String> = ["llama-3.1-8b".to_string()].into_iter().collect();
        let mut d = StreamDecoder::new().with_known_models(known);
        assert!(
            d.note_model_mismatch("typo-model", "llama-3.1-8b"),
            "an unknown requested id served as a different loaded model still warns"
        );
        assert!(
            !d.note_model_mismatch("typo-model", "llama-3.1-8b"),
            "deduped"
        );
    }

    #[test]
    fn mismatch_warning_text_is_canonical() {
        let mut d = StreamDecoder::new();
        let w = d.model_mismatch_warning("gpt-x", "gpt-y").expect("warns");
        assert_eq!(w, model_mismatch_text("gpt-x", "gpt-y"));
        assert!(w.contains("model mismatch"));
        assert!(
            d.model_mismatch_warning("gpt-x", "gpt-y").is_none(),
            "deduped"
        );
    }
}
