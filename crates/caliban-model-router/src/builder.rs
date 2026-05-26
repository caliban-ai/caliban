//! Fluent builder for [`crate::ModelRouter`].

use std::collections::HashMap;
use std::sync::Arc;

use caliban_provider::{Provider, RequestPurpose};

use crate::ModelRouter;
use crate::config::{BreakerPolicy, CapabilityRequirements, EffortMap, HedgePolicy, RouteEntry};
use crate::error::Result;

/// Fluent builder for [`ModelRouter`].
#[derive(Default)]
pub struct ModelRouterBuilder {
    pub(crate) default_purpose: Option<RequestPurpose>,
    pub(crate) routes: Vec<RouteEntry>,
    pub(crate) providers: HashMap<String, Arc<dyn Provider + Send + Sync>>,
}

impl std::fmt::Debug for ModelRouterBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRouterBuilder")
            .field("default_purpose", &self.default_purpose)
            .field("routes", &self.routes.len())
            .field("providers", &self.providers.len())
            .finish_non_exhaustive()
    }
}

impl ModelRouterBuilder {
    /// Set the purpose used when a request specifies none.
    #[must_use]
    pub fn default_purpose(mut self, p: RequestPurpose) -> Self {
        self.default_purpose = Some(p);
        self
    }

    /// Register a provider under a logical name.
    #[must_use]
    pub fn add_provider(
        mut self,
        name: impl Into<String>,
        provider: Arc<dyn Provider + Send + Sync>,
    ) -> Self {
        self.providers.insert(name.into(), provider);
        self
    }

    /// Append a route entry (simple form — sets defaults for the v2-only fields).
    #[must_use]
    pub fn route(
        mut self,
        purpose: RequestPurpose,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let provider = provider.into();
        let model = model.into();
        let id = format!(
            "{}:{}:{}",
            &provider,
            &model,
            match purpose {
                RequestPurpose::MainLoop => "main_loop",
                RequestPurpose::Summarization => "summarization",
                RequestPurpose::FastClassifier => "fast_classifier",
                RequestPurpose::SubAgent => "sub_agent",
                RequestPurpose::Embedding => "embedding",
                RequestPurpose::Other => "other",
            }
        );
        self.routes.push(RouteEntry {
            id,
            purpose,
            provider,
            model,
            requires: CapabilityRequirements::default(),
            fallback: None,
            hedge: HedgePolicy::Disabled,
            breaker: BreakerPolicy::disabled(),
            effort: None,
            effort_map: EffortMap::default(),
        });
        self
    }

    /// Append a fully-configured route.
    #[must_use]
    pub fn route_entry(mut self, entry: RouteEntry) -> Self {
        self.routes.push(entry);
        self
    }

    /// Build the router.
    ///
    /// # Errors
    /// See [`crate::RouterError`].
    pub fn build(self) -> Result<ModelRouter> {
        let default_purpose = self.default_purpose.unwrap_or(RequestPurpose::MainLoop);
        ModelRouter::new(default_purpose, self.routes, self.providers)
    }
}
