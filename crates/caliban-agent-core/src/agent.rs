//! Agent struct + builder + config.

use std::num::NonZeroUsize;
use std::sync::Arc;

use caliban_provider::{Provider, ThinkingConfig, ToolChoice};

use crate::error::{Error, Result};
use crate::hooks::{Hooks, NoopHooks};
use crate::registry::ToolRegistry;

/// Default per-turn parallel tool dispatch limit.
///
/// Returns `available_parallelism().get() - 1`, clamped to at least 1, so that
/// the agent loop, streaming, and the renderer can keep a core to themselves.
/// Falls back to 1 when `available_parallelism()` is unavailable.
#[must_use]
pub fn default_parallel_tool_limit() -> NonZeroUsize {
    let n = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(2)
        .saturating_sub(1)
        .max(1);
    NonZeroUsize::new(n).expect("max(1) guarantees nonzero")
}

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
    /// When true, mark the last system text block + last tool def with
    /// Anthropic-style `cache_control: Ephemeral`. No-op for other providers.
    pub(crate) prompt_cache: bool,
    /// When true, multiple `tool_use` blocks in one assistant turn run
    /// concurrently (bounded by `parallel_tool_limit`). When false, they
    /// run serially.
    pub(crate) parallel_tools: bool,
    /// Maximum concurrent tool invocations per turn. Ignored when
    /// `parallel_tools` is false (equivalent to `1`).
    pub(crate) parallel_tool_limit: NonZeroUsize,
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
pub struct AgentBuilder {
    provider: Option<Arc<dyn Provider + Send + Sync>>,
    tools: ToolRegistry,
    config: AgentConfig,
    compactor: Option<Arc<dyn crate::compact::Compactor + Send + Sync>>,
    retry: Option<crate::retry::RetryPolicy>,
    hooks: Option<Arc<dyn Hooks + Send + Sync>>,
    prompt_cache: bool,
    parallel_tools: bool,
    parallel_tool_limit: NonZeroUsize,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: None,
            tools: ToolRegistry::default(),
            config: AgentConfig::default(),
            compactor: None,
            retry: None,
            hooks: None,
            // Prompt caching is default-on. Anthropic users get cache hits
            // from turn 2 onward; non-Anthropic providers ignore the markers.
            prompt_cache: true,
            parallel_tools: true,
            parallel_tool_limit: default_parallel_tool_limit(),
        }
    }
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

    /// Enable or disable Anthropic-style prompt cache markers on the system
    /// prompt + last tool definition. Default: enabled.
    #[must_use]
    pub fn prompt_cache(mut self, on: bool) -> Self {
        self.prompt_cache = on;
        self
    }

    /// Enable or disable parallel tool dispatch. Default: enabled.
    ///
    /// When `false`, all `tool_use` blocks in a single assistant turn run
    /// serially in assistant-message order. When `true`, they run
    /// concurrently bounded by [`Self::parallel_tool_limit`].
    #[must_use]
    pub fn parallel_tools(mut self, on: bool) -> Self {
        self.parallel_tools = on;
        self
    }

    /// Set the maximum concurrent tool invocations per turn. Default:
    /// [`default_parallel_tool_limit()`] (typically `cores - 1`).
    #[must_use]
    pub fn parallel_tool_limit(mut self, limit: NonZeroUsize) -> Self {
        self.parallel_tool_limit = limit;
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
            prompt_cache: self.prompt_cache,
            parallel_tools: self.parallel_tools,
            parallel_tool_limit: self.parallel_tool_limit,
        })
    }
}

#[cfg(test)]
mod parallel_tools_config_tests {
    use super::*;

    #[test]
    fn default_limit_is_at_least_one() {
        let n = default_parallel_tool_limit();
        assert!(n.get() >= 1, "default cap must be >= 1");
    }

    #[test]
    fn default_limit_matches_cores_minus_one() {
        let cores = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(2);
        let expected = cores.saturating_sub(1).max(1);
        assert_eq!(default_parallel_tool_limit().get(), expected);
    }

    #[test]
    fn builder_defaults_parallel_tools_on() {
        let b = AgentBuilder::default();
        assert!(b.parallel_tools, "parallel_tools should default to true");
        assert!(b.parallel_tool_limit.get() >= 1);
    }

    #[test]
    fn builder_parallel_tools_setter() {
        let b = AgentBuilder::default().parallel_tools(false);
        assert!(!b.parallel_tools);
    }

    #[test]
    fn builder_parallel_tool_limit_setter() {
        let limit = std::num::NonZeroUsize::new(3).unwrap();
        let b = AgentBuilder::default().parallel_tool_limit(limit);
        assert_eq!(b.parallel_tool_limit.get(), 3);
    }
}
