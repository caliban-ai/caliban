//! Unit tests for `Agent::active_model` + `Agent::try_swap_model`
//! (same-provider hot swap; cross-provider deferred).

use std::sync::Arc;

use caliban_agent_core::{Agent, ModelSwapError};
use caliban_provider::MockProvider;

#[test]
fn active_model_starts_from_config_model() {
    let provider: Arc<MockProvider> = Arc::new(MockProvider::for_tests_with_models(&["model-A"]));
    let agent = Agent::builder()
        .provider(provider)
        .model("model-A")
        .max_tokens(64)
        .build()
        .expect("build agent");
    assert_eq!(agent.active_model().as_str(), "model-A");
}

#[test]
fn try_swap_model_same_provider_succeeds() {
    let provider: Arc<MockProvider> =
        Arc::new(MockProvider::for_tests_with_models(&["model-A", "model-B"]));
    let agent = Agent::builder()
        .provider(provider)
        .model("model-A")
        .max_tokens(64)
        .build()
        .expect("build agent");
    agent
        .try_swap_model("model-B")
        .expect("same-provider swap should succeed");
    assert_eq!(agent.active_model().as_str(), "model-B");
}

#[test]
fn try_swap_model_unsupported_returns_error() {
    let provider: Arc<MockProvider> = Arc::new(MockProvider::for_tests_with_models(&["model-A"]));
    let agent = Agent::builder()
        .provider(provider)
        .model("model-A")
        .max_tokens(64)
        .build()
        .expect("build agent");
    let err = agent.try_swap_model("never-heard-of-it").unwrap_err();
    assert!(
        matches!(err, ModelSwapError::UnsupportedByProvider(ref s) if s == "never-heard-of-it"),
        "got {err:?}"
    );
    assert_eq!(agent.active_model().as_str(), "model-A");
}
