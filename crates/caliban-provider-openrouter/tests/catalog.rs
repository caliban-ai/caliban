//! Catalogue behaviour — the reason this crate exists.
//!
//! The defect being fixed (#573) is that routing `OpenRouter` through the
//! `OpenAI` adapter silently answers `caps(128_000, 4_096, false, false)` for
//! every model. These tests pin the two halves of the fix: real capabilities
//! for catalogued models, and a *non-silent* miss for everything else.

use caliban_provider::{Provider, ToolUseCapability};
use caliban_provider_openrouter::{
    Catalog, OpenRouterProvider,
    catalog::{self, SCHEMA_VERSION},
    config,
};
use secrecy::SecretString;

fn provider_with(catalog: Catalog) -> OpenRouterProvider {
    let cfg = config::direct(SecretString::from("test-key"));
    OpenRouterProvider::with_catalog(cfg, catalog).expect("provider builds")
}

#[test]
fn embedded_catalogue_parses_and_is_populated() {
    let c = Catalog::embedded().expect("embedded snapshot parses");
    assert!(
        c.len() > 100,
        "embedded snapshot should carry the real catalogue, got {}",
        c.len()
    );
    assert!(!c.is_empty());
}

#[test]
fn capabilities_come_from_the_catalogue_not_a_blanket_default() {
    let c = Catalog::embedded().unwrap();
    // Find a catalogued model that advertises image input, and one that
    // advertises tools. The OpenAI fallback would deny both.
    let vision = c
        .model_infos()
        .into_iter()
        .find(|m| m.capabilities.vision)
        .expect("catalogue should contain at least one vision model");
    assert!(vision.capabilities.vision, "{} lost vision", vision.id);

    let tools = c
        .model_infos()
        .into_iter()
        .find(|m| m.capabilities.tool_use != ToolUseCapability::None)
        .expect("catalogue should contain at least one tool-using model");
    assert_ne!(tools.capabilities.tool_use, ToolUseCapability::None);
}

#[test]
fn catalogued_models_are_not_capped_at_the_openai_fallback() {
    let c = Catalog::embedded().unwrap();
    // The OpenAI fallback asserts max_output = 4096 for every unknown id.
    // At least one real OpenRouter model must exceed it, or the fix is moot.
    let best = c
        .model_infos()
        .into_iter()
        .map(|m| m.capabilities.max_output_tokens)
        .max()
        .unwrap_or(0);
    assert!(
        best > 4_096,
        "expected some model above the 4096 fallback, max was {best}"
    );
}

#[test]
fn unknown_model_yields_none_rather_than_a_guess() {
    let c = Catalog::embedded().unwrap();
    assert!(!c.contains("definitely/not-a-real-model"));
    assert!(
        c.capabilities_for("definitely/not-a-real-model").is_none(),
        "an uncatalogued model must not receive invented capabilities"
    );
}

#[test]
fn provider_reports_minimal_caps_for_an_unknown_model() {
    let p = provider_with(Catalog::embedded().unwrap());
    let caps = p.capabilities("definitely/not-a-real-model");
    // Every optional capability off — the opposite of the OpenAI fallback,
    // which asserts ParallelCalls + json_mode + Automatic caching.
    assert_eq!(caps.tool_use, ToolUseCapability::None);
    assert!(!caps.vision);
    assert!(!caps.thinking);
    assert!(!caps.json_mode);
}

#[test]
fn provider_name_is_openrouter() {
    let p = provider_with(Catalog::embedded().unwrap());
    assert_eq!(p.name(), "openrouter");
}

#[test]
fn list_models_is_sorted_and_matches_the_catalogue() {
    let c = Catalog::embedded().unwrap();
    let p = provider_with(c.clone());
    let listed = p.list_models();
    assert_eq!(listed.len(), c.len());
    let mut sorted = listed.clone();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(listed, sorted, "list_models must be deterministic");
}

#[test]
fn schema_version_mismatch_is_an_error_not_a_silent_empty_catalogue() {
    let raw = format!(
        r#"{{"schema_version": {}, "models": []}}"#,
        SCHEMA_VERSION + 99
    );
    let got = Catalog::from_snapshot_json(&raw);
    assert!(
        matches!(got, Err(catalog::CatalogError::SchemaVersion { .. })),
        "a future schema must fail loudly, got {got:?}"
    );
}

#[test]
fn an_operator_supplied_snapshot_is_loaded_from_disk() {
    let dir = std::env::temp_dir().join(format!("or-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("models.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"schema_version": {SCHEMA_VERSION}, "models": [
                {{"id":"acme/from-disk","context_length":4096,
                  "max_completion_tokens":1024,"input_modalities":["text"],
                  "supported_parameters":["tools"],"pricing":{{}}}}
            ]}}"#
        ),
    )
    .unwrap();

    let c = Catalog::from_path(&path).expect("operator snapshot loads");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(c.len(), 1);
    assert_eq!(
        c.capabilities_for("acme/from-disk").unwrap().tool_use,
        ToolUseCapability::ParallelCalls
    );
}

#[test]
fn an_unreadable_operator_snapshot_is_an_error_not_a_silent_fallback() {
    let missing = std::env::temp_dir().join("or-definitely-missing-snapshot.json");
    let _ = std::fs::remove_file(&missing);
    assert!(
        matches!(
            Catalog::from_path(&missing),
            Err(catalog::CatalogError::Read { .. })
        ),
        "a bad override must not silently fall back to the embedded snapshot"
    );
}

#[test]
fn live_json_shape_maps_onto_the_same_capabilities() {
    // The upstream payload nests what the snapshot flattens; both paths must
    // produce identical capabilities or `refresh_models` would disagree with
    // the vendored view.
    let live = r#"{"data":[{
        "id":"acme/thinker",
        "context_length":200000,
        "top_provider":{"max_completion_tokens":64000},
        "architecture":{"input_modalities":["text","image"]},
        "supported_parameters":["tools","reasoning","response_format","top_k","stop"],
        "pricing":{"prompt":"0.000003","completion":"0.000015"}
    }]}"#;
    let c = Catalog::from_live_json(live).expect("live shape parses");
    let caps = c.capabilities_for("acme/thinker").expect("model present");

    assert_eq!(caps.max_input_tokens, 200_000);
    assert_eq!(caps.max_output_tokens, 64_000);
    assert!(caps.vision);
    assert!(caps.thinking);
    assert!(caps.json_mode);
    assert!(caps.top_k);
    assert!(caps.stop_sequences);
    assert_eq!(caps.tool_use, ToolUseCapability::ParallelCalls);

    let entry = c.get("acme/thinker").unwrap();
    assert!((entry.pricing.input_per_mtok().unwrap() - 3.0).abs() < 1e-9);
    assert!((entry.pricing.output_per_mtok().unwrap() - 15.0).abs() < 1e-9);
}

#[test]
fn missing_max_completion_tokens_falls_back_to_the_context_window() {
    let live = r#"{"data":[{
        "id":"acme/no-cap",
        "context_length":128000,
        "top_provider":{},
        "architecture":{"input_modalities":["text"]},
        "supported_parameters":[]
    }]}"#;
    let c = Catalog::from_live_json(live).unwrap();
    let caps = c.capabilities_for("acme/no-cap").unwrap();
    assert_eq!(caps.max_output_tokens, 128_000);
    assert_eq!(caps.tool_use, ToolUseCapability::None);
    assert!(!caps.vision);
}
