//! [`Provider`] implementation for [`ModelRouter`].
//!
//! The heavy lifting (candidate dispatch, fallback, hedging) lives in
//! [`crate::dispatch`]; this module wires the trait methods through to
//! those helpers and supplies the `capabilities` / `list_models` / `name`
//! glue that doesn't need a dispatch loop.

use std::collections::HashMap;

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, MessageStream, ModelInfo, Provider,
    error::Result as ProviderResult,
};

use crate::ModelRouter;

#[async_trait]
impl Provider for ModelRouter {
    async fn complete(&self, request: CompletionRequest) -> ProviderResult<CompletionResponse> {
        self.dispatch_complete(request).await
    }

    async fn stream(&self, request: CompletionRequest) -> ProviderResult<MessageStream> {
        self.dispatch_stream(request).await
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        // Pick a route matching the default purpose, then ask its provider.
        let route = self
            .routes
            .iter()
            .find(|r| r.purpose == self.default_purpose)
            .expect("router build validates default_purpose has a route");
        let provider = self
            .providers
            .get(&route.provider)
            .expect("router build validates provider names");
        let m = if model.is_empty() {
            route.model.as_str()
        } else {
            model
        };
        provider.capabilities(m)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        let mut by_id: HashMap<String, ModelInfo> = HashMap::new();
        for p in self.providers.values() {
            for m in p.list_models() {
                by_id.entry(m.id.clone()).or_insert(m);
            }
        }
        by_id.into_values().collect()
    }

    fn name(&self) -> &'static str {
        "router"
    }
}
