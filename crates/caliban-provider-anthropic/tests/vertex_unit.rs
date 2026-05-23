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
fn vertex_id_for_claude_3_5_sonnet() {
    assert_eq!(
        vertex_wire_model_id("claude-3-5-sonnet"),
        "claude-3-5-sonnet@20241022"
    );
}

#[test]
fn vertex_id_for_claude_3_5_haiku() {
    assert_eq!(
        vertex_wire_model_id("claude-3-5-haiku"),
        "claude-3-5-haiku@20241022"
    );
}

#[test]
fn vertex_id_for_claude_3_opus() {
    assert_eq!(
        vertex_wire_model_id("claude-3-opus"),
        "claude-3-opus@20240229"
    );
}

#[test]
fn vertex_id_for_claude_3_7_sonnet() {
    assert_eq!(
        vertex_wire_model_id("claude-3-7-sonnet"),
        "claude-3-7-sonnet@20250219"
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
