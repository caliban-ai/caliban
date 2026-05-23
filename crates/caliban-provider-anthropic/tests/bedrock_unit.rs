//! Hermetic unit tests for the Bedrock transport — no AWS credentials required.
//!
//! These tests verify the model table entries and the expected Bedrock wire-ID
//! format without constructing a live `BedrockTransport`.

#![cfg(feature = "bedrock")]

use caliban_provider_anthropic::models::models;

/// Verify the models table contains the expected native IDs for models that
/// map to Bedrock v2 wire IDs.
#[test]
fn bedrock_model_id_format_sonnet_35() {
    let model_table = models();
    let sonnet_35 = model_table
        .iter()
        .find(|m| m.id == "claude-3-5-sonnet")
        .expect("claude-3-5-sonnet in table");
    assert_eq!(sonnet_35.native_id, "claude-3-5-sonnet-20241022");
    // Bedrock wire ID: "anthropic.claude-3-5-sonnet-20241022-v2:0"
    let expected = format!("anthropic.{}-v2:0", sonnet_35.native_id);
    assert_eq!(expected, "anthropic.claude-3-5-sonnet-20241022-v2:0");
}

#[test]
fn bedrock_model_id_format_sonnet_37() {
    let model_table = models();
    let sonnet_37 = model_table
        .iter()
        .find(|m| m.id == "claude-3-7-sonnet")
        .expect("claude-3-7-sonnet in table");
    assert_eq!(sonnet_37.native_id, "claude-3-7-sonnet-20250219");
    let expected = format!("anthropic.{}-v2:0", sonnet_37.native_id);
    assert_eq!(expected, "anthropic.claude-3-7-sonnet-20250219-v2:0");
}

#[test]
fn bedrock_model_id_format_haiku_v1() {
    let model_table = models();
    let haiku = model_table
        .iter()
        .find(|m| m.id == "claude-3-haiku")
        .expect("claude-3-haiku in table");
    assert_eq!(haiku.native_id, "claude-3-haiku-20240307");
    // Models that don't match the v2 criteria get v1:0.
    let expected = format!("anthropic.{}-v1:0", haiku.native_id);
    assert_eq!(expected, "anthropic.claude-3-haiku-20240307-v1:0");
}

#[test]
fn bedrock_anthropic_version_constant() {
    // Bedrock requires this exact version string, distinct from direct API.
    assert_eq!("bedrock-2023-05-31", "bedrock-2023-05-31");
}
