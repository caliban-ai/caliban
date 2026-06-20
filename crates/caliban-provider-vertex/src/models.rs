//! Vertex model catalog + control-plane list helper.

use std::sync::Arc;

use caliban_provider::ModelInfo;
use caliban_provider_anthropic::models::{capabilities_for, models as anthropic_models};
use gcp_auth::TokenProvider;
use serde::Deserialize;

use crate::error::VertexError;

const GCP_SCOPE: &[&str] = &["https://www.googleapis.com/auth/cloud-platform"];

/// Strip the Vertex `@<date>` suffix from a wire model id to recover the
/// canonical base model.
///
/// Examples:
/// - `claude-3-5-sonnet@20241022` → `claude-3-5-sonnet`
/// - `claude-3-5-sonnet` → `claude-3-5-sonnet` (passthrough)
#[must_use]
pub fn strip_platform_suffix(model: &str) -> String {
    model
        .split_once('@')
        .map_or_else(|| model.to_string(), |(base, _)| base.to_string())
}

/// Capabilities lookup that strips the Vertex `@<date>` suffix.
#[must_use]
pub fn capabilities_for_vertex(model: &str) -> caliban_provider::Capabilities {
    capabilities_for(&strip_platform_suffix(model))
}

/// Vendored fallback list of Anthropic Claude models known to be served by
/// Vertex AI. Mirrors `caliban_provider_anthropic::models::models()` with
/// `native_id` rewritten to the Vertex wire format.
#[must_use]
pub fn vendored_vertex_models() -> Vec<ModelInfo> {
    anthropic_models()
        .into_iter()
        .map(|mut m| {
            m.native_id = to_vertex_wire_id(&m.native_id);
            m
        })
        .collect()
}

/// Compute the Vertex wire id for an Anthropic native model id. Mirrors
/// the logic in `VertexTransport::wire_model_id`.
fn to_vertex_wire_id(native_id: &str) -> String {
    if native_id.contains('@') {
        return native_id.to_string();
    }
    if let Some(i) = native_id.rfind('-') {
        let (prefix, suffix) = native_id.split_at(i);
        let after_dash = &suffix[1..];
        if after_dash.len() == 8 && after_dash.chars().all(|c| c.is_ascii_digit()) {
            return format!("{prefix}@{after_dash}");
        }
    }
    native_id.to_string()
}

#[derive(Deserialize)]
struct VertexModelsResponse {
    #[serde(default)]
    models: Vec<VertexModelRow>,
    #[serde(default)]
    publisher_models: Vec<VertexModelRow>,
}

#[derive(Deserialize)]
struct VertexModelRow {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
}

/// Endpoint used by [`list_models_remote`].
pub(crate) fn list_models_endpoint(base_url: &str) -> String {
    format!("{base_url}/v1/publishers/anthropic/models")
}

/// Default base URL builder used in production.
pub(crate) fn default_base_url(region: &str) -> String {
    format!("https://{region}-aiplatform.googleapis.com")
}

/// Fetch the live `publishers/anthropic/models` list from Vertex AI.
///
/// Returns [`VertexError`] on a token/auth failure, a network error, or a
/// non-2xx response — it does **not** fall back to the vendored list itself;
/// the caller decides whether to (there is no production caller yet —
/// `refresh_models` is test-only). Successful responses are filtered to models
/// whose `name` starts with `publishers/anthropic/models/`.
pub async fn list_models_remote(
    client: &reqwest::Client,
    token_provider: &Arc<dyn TokenProvider>,
    base_url: &str,
) -> std::result::Result<Vec<ModelInfo>, VertexError> {
    let url = list_models_endpoint(base_url);
    let token = token_provider
        .token(GCP_SCOPE)
        .await
        .map_err(VertexError::Auth)?;
    let resp = client.get(&url).bearer_auth(token.as_str()).send().await?;
    let status = resp.status();
    let body = resp.bytes().await?;
    if !status.is_success() {
        return Err(VertexError::InvalidConfig(format!(
            "list_models {url} returned status {status}"
        )));
    }
    parse_models_response(&body)
}

pub(crate) fn parse_models_response(
    body: &[u8],
) -> std::result::Result<Vec<ModelInfo>, VertexError> {
    let parsed: VertexModelsResponse = serde_json::from_slice(body)?;
    let rows = if parsed.models.is_empty() {
        parsed.publisher_models
    } else {
        parsed.models
    };
    let known: Vec<ModelInfo> = anthropic_models();
    let mut out = Vec::new();
    for row in rows {
        // Vertex returns names like `publishers/anthropic/models/claude-3-5-sonnet@20241022`.
        let Some(short) = row
            .name
            .strip_prefix("publishers/anthropic/models/")
            .or_else(|| row.name.rsplit('/').next())
        else {
            continue;
        };
        let canonical = strip_platform_suffix(short);
        let caps = capabilities_for(&canonical);
        let display = row.display_name.unwrap_or_else(|| canonical.clone());
        // Prefer the canonical id from the table when we recognize it.
        let id = known
            .iter()
            .find(|m| m.native_id.starts_with(&canonical) || m.id == canonical)
            .map_or(canonical, |m| m.id.clone());
        out.push(ModelInfo {
            id,
            native_id: short.to_string(),
            display_name: display,
            capabilities: caps,
        });
    }
    if out.is_empty() {
        return Ok(vendored_vertex_models());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_platform_suffix_drops_at_suffix() {
        assert_eq!(
            strip_platform_suffix("claude-3-5-sonnet@20241022"),
            "claude-3-5-sonnet"
        );
        assert_eq!(strip_platform_suffix("custom"), "custom");
    }

    #[test]
    fn vendored_vertex_models_pass_through_dateless_ids() {
        let models = vendored_vertex_models();
        // Claude 4.x native IDs are dateless; the Vertex wire-id helper
        // passes them through unchanged.
        let sonnet = models
            .iter()
            .find(|m| m.id == "claude-sonnet-4-6")
            .expect("sonnet-4-6 present");
        assert_eq!(sonnet.native_id, "claude-sonnet-4-6");
    }

    #[test]
    fn parse_models_response_canonical_publishers_path() {
        let body = serde_json::json!({
            "models": [
                {
                    "name": "publishers/anthropic/models/claude-sonnet-4-6@20260101",
                    "display_name": "Claude Sonnet 4.6"
                },
                {
                    "name": "publishers/anthropic/models/claude-haiku-4-5@20251001",
                    "display_name": "Claude Haiku 4.5"
                }
            ]
        });
        let s = serde_json::to_vec(&body).unwrap();
        let parsed = parse_models_response(&s).expect("parse");
        assert_eq!(parsed.len(), 2);
        let sonnet = parsed
            .iter()
            .find(|m| m.id == "claude-sonnet-4-6")
            .expect("sonnet-4-6 present");
        assert_eq!(sonnet.native_id, "claude-sonnet-4-6@20260101");
        assert_eq!(sonnet.display_name, "Claude Sonnet 4.6");
    }

    #[test]
    fn parse_models_response_empty_falls_back() {
        let body = serde_json::json!({ "models": [] });
        let s = serde_json::to_vec(&body).unwrap();
        let parsed = parse_models_response(&s).expect("parse");
        // Empty → vendored fallback (non-empty)
        assert!(!parsed.is_empty());
    }
}
