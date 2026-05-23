//! Agent struct + builder + config.

use std::sync::Arc;

use caliban_provider::{Provider, ThinkingConfig, ToolChoice};

use crate::error::{Error, Result};
use crate::hooks::{Hooks, NoopHooks};
use crate::registry::ToolRegistry;

/// Per-turn settings that control how the agent interacts with the provider.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The model identifier string forwarded to the provider (e.g. `"claude-opus-4-5"`).
    pub model: String,
    /// Maximum number of tokens the model may generate per turn.
    pub max_tokens: u32,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    /// Optional nucleus-sampling probability.
    pub top_p: Option<f32>,
    /// Sequences that stop generation when produced.
    pub stop_sequences: Vec<String>,
    /// Optional extended-thinking configuration.
    pub thinking: Option<ThinkingConfig>,
    /// Optional opaque user identifier forwarded to the provider.
    pub user_id: Option<String>,
    /// Maximum number of agentic turns before returning `Error::MaxTurnsReached`.
    pub max_turns: u32,
    /// Tool-choice policy sent to the provider.
    pub tool_choice: ToolChoice,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            stop_sequences: Vec::new(),
            thinking: None,
            user_id: None,
            max_turns: 50,
            tool_choice: ToolChoice::default(),
        }
    }
}

/// The stateless agent: a provider + tools + config + compactor + retry + hooks.
///
/// Construct via [`AgentBuilder`] (`Agent::builder()`). The turn loop itself is
/// added in subsequent tasks; this struct exposes the API surface and configuration
/// accessors.
pub struct Agent {
    pub(crate) provider: Arc<dyn Provider + Send + Sync>,
    pub(crate) tools: ToolRegistry,
    pub(crate) config: AgentConfig,
    pub(crate) compactor: Arc<dyn crate::compact::Compactor + Send + Sync>,
    pub(crate) retry: crate::retry::RetryPolicy,
    pub(crate) hooks: Arc<dyn Hooks + Send + Sync>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("provider", &self.provider.name())
            .field("tools", &self.tools)
            .field("config", &self.config)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Return a new [`AgentBuilder`].
    #[must_use]
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    /// Return a reference to the agent's configuration.
    #[must_use]
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Return a reference to the agent's tool registry.
    #[must_use]
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }
}

/// Fluent builder for [`Agent`].
///
/// Call [`Agent::builder()`] to obtain one. All setter methods consume and
/// return `self` so calls can be chained. [`AgentBuilder::build`] finalises
/// construction with required-field validation.
#[derive(Default)]
pub struct AgentBuilder {
    provider: Option<Arc<dyn Provider + Send + Sync>>,
    tools: ToolRegistry,
    config: AgentConfig,
    compactor: Option<Arc<dyn crate::compact::Compactor + Send + Sync>>,
    retry: Option<crate::retry::RetryPolicy>,
    hooks: Option<Arc<dyn Hooks + Send + Sync>>,
}

impl AgentBuilder {
    /// Set the provider that the agent will call.
    #[must_use]
    pub fn provider(mut self, p: Arc<dyn Provider + Send + Sync>) -> Self {
        self.provider = Some(p);
        self
    }

    /// Set the tool registry.
    #[must_use]
    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    /// Set the full agent configuration, replacing the current one.
    #[must_use]
    pub fn config(mut self, cfg: AgentConfig) -> Self {
        self.config = cfg;
        self
    }

    /// Set the model identifier (convenience shorthand for `.config.model`).
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set the maximum number of tokens per turn.
    #[must_use]
    pub fn max_tokens(mut self, n: u32) -> Self {
        self.config.max_tokens = n;
        self
    }

    /// Set the maximum number of agentic turns.
    #[must_use]
    pub fn max_turns(mut self, n: u32) -> Self {
        self.config.max_turns = n;
        self
    }

    /// Set the sampling temperature.
    #[must_use]
    pub fn temperature(mut self, t: f32) -> Self {
        self.config.temperature = Some(t);
        self
    }

    /// Set the compaction strategy. Defaults to [`crate::compact::NoopCompactor`].
    #[must_use]
    pub fn compactor(mut self, c: Arc<dyn crate::compact::Compactor + Send + Sync>) -> Self {
        self.compactor = Some(c);
        self
    }

    /// Set the retry policy. Defaults to [`crate::retry::RetryPolicy::default()`].
    #[must_use]
    pub fn retry_policy(mut self, p: crate::retry::RetryPolicy) -> Self {
        self.retry = Some(p);
        self
    }

    /// Set the lifecycle hooks. Defaults to [`crate::hooks::NoopHooks`].
    #[must_use]
    pub fn hooks(mut self, h: Arc<dyn Hooks + Send + Sync>) -> Self {
        self.hooks = Some(h);
        self
    }

    /// Finalise the builder, validating required fields.
    ///
    /// # Errors
    /// Returns `Error::Misconfigured` if no provider was set, the model string
    /// is empty, or `max_tokens` is zero.
    pub fn build(self) -> Result<Agent> {
        let provider = self
            .provider
            .ok_or_else(|| Error::Misconfigured("Agent::provider is required".into()))?;
        if self.config.model.is_empty() {
            return Err(Error::Misconfigured("Agent::model is required".into()));
        }
        if self.config.max_tokens == 0 {
            return Err(Error::Misconfigured("Agent::max_tokens must be > 0".into()));
        }
        Ok(Agent {
            provider,
            tools: self.tools,
            config: self.config,
            compactor: self
                .compactor
                .unwrap_or_else(|| Arc::new(crate::compact::NoopCompactor)),
            retry: self.retry.unwrap_or_default(),
            hooks: self.hooks.unwrap_or_else(|| Arc::new(NoopHooks)),
        })
    }
}
