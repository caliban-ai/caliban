//! Hermetic tests for `BedrockProvider` — no AWS credentials required.

#![allow(missing_docs)]
#![allow(clippy::duration_suboptimal_units)]

use std::collections::HashMap;
use std::time::Duration;

use caliban_provider::{Capabilities, Provider};
use caliban_provider_bedrock::{
    AuthRefresh, BedrockConfig, BedrockError, BedrockProvider,
    models::{strip_platform_suffix, vendored_bedrock_models},
};

/// Build a closure that pretends to be `std::env::var` returning a small
/// fixed map.
fn env_map(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

// ---------------------------------------------------------------------------
// Config tests
// ---------------------------------------------------------------------------

#[test]
fn config_from_env_requires_region() {
    let getter = env_map(&[]);
    let err = BedrockConfig::from_env_fn(&getter).unwrap_err();
    assert!(matches!(err, BedrockError::MissingConfig("AWS_REGION")));
}

#[test]
fn config_from_env_reads_fields() {
    let getter = env_map(&[
        ("AWS_REGION", "us-west-2"),
        (
            "BEDROCK_INFERENCE_PROFILE_ID",
            "us.anthropic.claude-3-5-sonnet-20241022-v2:0",
        ),
        ("AWS_PROFILE", "caliban-prod"),
        ("CALIBAN_AWS_AUTH_REFRESH", "2m"),
    ]);
    let cfg = BedrockConfig::from_env_fn(&getter).expect("config");
    assert_eq!(cfg.region, "us-west-2");
    assert_eq!(
        cfg.inference_profile_id.as_deref(),
        Some("us.anthropic.claude-3-5-sonnet-20241022-v2:0")
    );
    assert_eq!(cfg.aws_profile.as_deref(), Some("caliban-prod"));
    assert_eq!(cfg.auth_refresh, Duration::from_mins(2));
}

#[test]
fn config_default_has_5min_refresh() {
    let cfg = BedrockConfig::default();
    assert_eq!(cfg.auth_refresh, Duration::from_mins(5));
}

#[test]
fn config_invalid_refresh_duration_errors() {
    let getter = env_map(&[
        ("AWS_REGION", "us-west-2"),
        ("CALIBAN_AWS_AUTH_REFRESH", "nope"),
    ]);
    let err = BedrockConfig::from_env_fn(&getter).unwrap_err();
    assert!(matches!(err, BedrockError::InvalidConfig(_)));
}

#[test]
fn config_falls_back_to_aws_default_region() {
    let getter = env_map(&[("AWS_DEFAULT_REGION", "eu-central-1")]);
    let cfg = BedrockConfig::from_env_fn(&getter).expect("config");
    assert_eq!(cfg.region, "eu-central-1");
}

// ---------------------------------------------------------------------------
// Auth refresh
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn auth_refresh_ticks_on_interval() {
    let auth = AuthRefresh::spawn(Duration::from_mins(1));
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_secs(125)).await;
    assert!(
        auth.refresh_count() >= 1,
        "expected at least one tick, got {}",
        auth.refresh_count()
    );
}

// ---------------------------------------------------------------------------
// Provider behavior
// ---------------------------------------------------------------------------

fn test_config() -> BedrockConfig {
    BedrockConfig {
        region: "us-west-2".into(),
        inference_profile_id: None,
        aws_profile: None,
        endpoint_override: None,
        auth_refresh: Duration::ZERO,
    }
}

#[tokio::test]
async fn provider_name_returns_bedrock() {
    let p = BedrockProvider::from_config(test_config()).await.unwrap();
    assert_eq!(p.name(), "bedrock");
}

#[tokio::test]
async fn provider_capabilities_match_anthropic() {
    let p = BedrockProvider::from_config(test_config()).await.unwrap();
    let bedrock_caps: Capabilities = p.capabilities("anthropic.claude-sonnet-4-6-v1:0");
    let anthropic_caps = caliban_provider_anthropic::models::capabilities_for("claude-sonnet-4-6");
    assert_eq!(bedrock_caps, anthropic_caps);
    assert!(bedrock_caps.vision);
}

#[tokio::test]
async fn provider_list_models_filters_to_anthropic() {
    let p = BedrockProvider::from_config(test_config()).await.unwrap();
    let models = p.list_models();
    assert!(!models.is_empty());
    for m in &models {
        assert!(
            m.native_id.starts_with("anthropic."),
            "{} should start with anthropic.",
            m.native_id
        );
    }
}

#[test]
fn strip_platform_suffix_drops_region_and_version() {
    assert_eq!(
        strip_platform_suffix("us.anthropic.claude-3-7-sonnet-20250219-v1:0"),
        "claude-3-7-sonnet"
    );
    assert_eq!(
        strip_platform_suffix("anthropic.claude-3-5-sonnet-20241022-v2:0"),
        "claude-3-5-sonnet"
    );
}

#[test]
fn vendored_bedrock_models_format() {
    let models = vendored_bedrock_models();
    let haiku = models
        .iter()
        .find(|m| m.id == "claude-haiku-4-5")
        .expect("haiku-4-5 present");
    assert_eq!(haiku.native_id, "anthropic.claude-haiku-4-5-v1:0");
}

/// Live AWS Bedrock test. Only runs if `CALIBAN_LIVE_BEDROCK=1`.
#[tokio::test]
#[ignore = "requires real AWS credentials and CALIBAN_LIVE_BEDROCK=1"]
async fn live_bedrock_smoke() {
    if std::env::var("CALIBAN_LIVE_BEDROCK").ok().as_deref() != Some("1") {
        return;
    }
    let cfg = BedrockConfig::from_env().expect("env");
    let _provider = BedrockProvider::from_config(cfg).await.expect("provider");
}
