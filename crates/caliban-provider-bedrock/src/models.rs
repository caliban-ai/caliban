//! Bedrock model catalog.
//!
//! Per ADR 0034 the canonical model names live in
//! `caliban-provider-anthropic`. This module exposes a vendored list of
//! Anthropic models known to be served by Bedrock and a helper to map a
//! Bedrock wire model ID back to its canonical base model so
//! `Capabilities` lookups can reuse the Anthropic table.
//!
//! A future ADR will add a control-plane refresh path via
//! `aws-sdk-bedrock::ListInferenceProfiles`; until then we ship a
//! deterministic static list.

use caliban_provider::ModelInfo;
use caliban_provider_anthropic::models::{capabilities_for, models as anthropic_models};

/// Strip the Bedrock platform prefix/suffix from a wire model id to recover
/// the canonical base model.
///
/// Examples:
/// - `anthropic.claude-3-5-sonnet-20241022-v2:0` →
///   `claude-3-5-sonnet`
/// - `us.anthropic.claude-3-7-sonnet-20250219-v1:0` →
///   `claude-3-7-sonnet`
/// - `claude-3-5-sonnet` → `claude-3-5-sonnet` (passthrough)
#[must_use]
pub fn strip_platform_suffix(model: &str) -> String {
    // Strip optional region prefix like "us." / "eu." that AWS adds to
    // inference profile ids.
    let mut rest = model;
    if let Some((prefix, after)) = model.split_once('.')
        && prefix.len() <= 3
        && prefix.chars().all(|c| c.is_ascii_alphabetic())
    {
        rest = after;
    }
    // Drop the "anthropic." publisher prefix if present.
    let rest = rest.strip_prefix("anthropic.").unwrap_or(rest);
    // Drop the trailing version tag (`-v1:0`, `-v2:0`).
    let rest = rest.split(":v").next().unwrap_or(rest);
    let rest = match rest.rfind("-v") {
        Some(i)
            if rest[i + 2..]
                .chars()
                .all(|c| c.is_ascii_digit() || c == ':') =>
        {
            &rest[..i]
        }
        _ => rest,
    };
    // Drop the trailing 8-digit date if present.
    if let Some(i) = rest.rfind('-')
        && rest[i + 1..].len() == 8
        && rest[i + 1..].chars().all(|c| c.is_ascii_digit())
    {
        return rest[..i].to_string();
    }
    rest.to_string()
}

/// Capabilities lookup that strips the Bedrock wire prefix/suffix.
#[must_use]
pub fn capabilities_for_bedrock(model: &str) -> caliban_provider::Capabilities {
    capabilities_for(&strip_platform_suffix(model))
}

/// Vendored list of Anthropic Claude models known to be served by Bedrock.
///
/// Mirrors `caliban_provider_anthropic::models::models()` with `native_id`
/// rewritten to the Bedrock wire format.
#[must_use]
pub fn vendored_bedrock_models() -> Vec<ModelInfo> {
    anthropic_models()
        .into_iter()
        .map(|mut m| {
            m.native_id = to_bedrock_wire_id(&m.native_id);
            m
        })
        .collect()
}

/// Compute the Bedrock wire ID for an Anthropic native model id. Mirrors
/// the logic in `BedrockTransport::wire_model_id`.
fn to_bedrock_wire_id(native_id: &str) -> String {
    if native_id.starts_with("anthropic.") {
        return native_id.to_string();
    }
    let suffix = if native_id.contains("3-5-sonnet-20241022") || native_id.contains("3-7-sonnet") {
        "v2:0"
    } else {
        "v1:0"
    };
    format!("anthropic.{native_id}-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_platform_suffix_full_wire_id_v2() {
        assert_eq!(
            strip_platform_suffix("anthropic.claude-3-5-sonnet-20241022-v2:0"),
            "claude-3-5-sonnet"
        );
    }

    #[test]
    fn strip_platform_suffix_with_region_prefix() {
        assert_eq!(
            strip_platform_suffix("us.anthropic.claude-3-7-sonnet-20250219-v1:0"),
            "claude-3-7-sonnet"
        );
    }

    #[test]
    fn strip_platform_suffix_passthrough_canonical() {
        assert_eq!(
            strip_platform_suffix("claude-3-5-sonnet"),
            "claude-3-5-sonnet"
        );
    }

    #[test]
    fn vendored_bedrock_models_have_wire_ids() {
        let models = vendored_bedrock_models();
        let sonnet = models
            .iter()
            .find(|m| m.id == "claude-3-5-sonnet")
            .expect("3-5-sonnet present");
        assert_eq!(
            sonnet.native_id,
            "anthropic.claude-3-5-sonnet-20241022-v2:0"
        );
        let haiku = models
            .iter()
            .find(|m| m.id == "claude-3-haiku")
            .expect("3-haiku present");
        assert_eq!(haiku.native_id, "anthropic.claude-3-haiku-20240307-v1:0");
    }

    #[test]
    fn capabilities_for_bedrock_matches_anthropic() {
        let bedrock_caps = capabilities_for_bedrock("anthropic.claude-3-5-sonnet-20241022-v2:0");
        let anthropic_caps = capabilities_for("claude-3-5-sonnet");
        assert_eq!(bedrock_caps, anthropic_caps);
    }
}
