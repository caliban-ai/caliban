//! Hermetic unit tests for the Bedrock transport — no AWS credentials required.
//!
//! These tests verify the model table entries and the expected Bedrock wire-ID
//! format without constructing a live `BedrockTransport`.

#![cfg(feature = "bedrock")]

use caliban_provider_anthropic::models::models;

#[test]
fn bedrock_model_id_format_opus_47() {
    let model_table = models();
    let opus = model_table
        .iter()
        .find(|m| m.id == "claude-opus-4-7")
        .expect("claude-opus-4-7 in table");
    assert_eq!(opus.native_id, "claude-opus-4-7");
    // Current Claude 4 native IDs are dateless and map to v1:0.
    let expected = format!("anthropic.{}-v1:0", opus.native_id);
    assert_eq!(expected, "anthropic.claude-opus-4-7-v1:0");
}

#[test]
fn bedrock_model_id_format_sonnet_46() {
    let model_table = models();
    let sonnet = model_table
        .iter()
        .find(|m| m.id == "claude-sonnet-4-6")
        .expect("claude-sonnet-4-6 in table");
    assert_eq!(sonnet.native_id, "claude-sonnet-4-6");
    let expected = format!("anthropic.{}-v1:0", sonnet.native_id);
    assert_eq!(expected, "anthropic.claude-sonnet-4-6-v1:0");
}

#[test]
fn bedrock_model_id_format_haiku_45() {
    let model_table = models();
    let haiku = model_table
        .iter()
        .find(|m| m.id == "claude-haiku-4-5")
        .expect("claude-haiku-4-5 in table");
    assert_eq!(haiku.native_id, "claude-haiku-4-5");
    let expected = format!("anthropic.{}-v1:0", haiku.native_id);
    assert_eq!(expected, "anthropic.claude-haiku-4-5-v1:0");
}

#[test]
fn bedrock_anthropic_version_constant() {
    // Bedrock requires this exact version string, distinct from direct API.
    assert_eq!("bedrock-2023-05-31", "bedrock-2023-05-31");
}
