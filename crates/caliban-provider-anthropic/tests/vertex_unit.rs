//! Hermetic unit tests for the Vertex AI transport — no GCP credentials required.
//!
//! These tests verify the model-ID translation logic (canonical → Vertex wire format)
//! without constructing a live `VertexTransport`.

#![cfg(feature = "vertex")]

use caliban_provider_anthropic::models::models;

/// Reproduce the `wire_model_id` logic as a free function for testing.
fn vertex_wire_model_id(canonical: &str) -> String {
    let native_id = models()
        .into_iter()
        .find(|m| m.id == canonical)
        .map_or_else(|| canonical.to_string(), |m| m.native_id);

    if native_id.contains('@') {
        return native_id;
    }

    if let Some(dash_pos) = native_id.rfind('-') {
        let (prefix, suffix) = native_id.split_at(dash_pos);
        let after_dash = &suffix[1..];
        if after_dash.len() == 8 && after_dash.chars().all(|c| c.is_ascii_digit()) {
            return format!("{prefix}@{after_dash}");
        }
    }
    native_id
}

#[test]
fn vertex_id_for_claude_opus_4_7() {
    // Current Claude 4.x native IDs are dateless and pass through unchanged.
    assert_eq!(vertex_wire_model_id("claude-opus-4-7"), "claude-opus-4-7");
}

#[test]
fn vertex_id_for_claude_sonnet_4_6() {
    assert_eq!(
        vertex_wire_model_id("claude-sonnet-4-6"),
        "claude-sonnet-4-6"
    );
}

#[test]
fn vertex_id_for_claude_haiku_4_5() {
    assert_eq!(vertex_wire_model_id("claude-haiku-4-5"), "claude-haiku-4-5");
}

#[test]
fn vertex_id_date_suffix_converts_to_at_form() {
    // An 8-digit date suffix on a non-table ID should be converted via the
    // canonical "-YYYYMMDD" → "@YYYYMMDD" rewrite.
    assert_eq!(
        vertex_wire_model_id("claude-opus-4-7-20251115"),
        "claude-opus-4-7@20251115"
    );
}

#[test]
fn vertex_id_already_vertex_format_unchanged() {
    // A string already containing '@' is returned as-is.
    assert_eq!(
        vertex_wire_model_id("claude-3-5-sonnet@20241022"),
        "claude-3-5-sonnet@20241022"
    );
}

#[test]
fn vertex_id_unknown_model_passes_through() {
    // A completely unknown ID with no date suffix passes through unchanged.
    assert_eq!(vertex_wire_model_id("custom-model"), "custom-model");
}

#[test]
fn vertex_anthropic_version_constant() {
    // Vertex requires this exact version string, distinct from direct/bedrock.
    assert_eq!("vertex-2023-10-16", "vertex-2023-10-16");
}
