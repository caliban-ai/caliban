//! Agent struct + builder + config.

use std::num::NonZeroUsize;
use std::sync::Arc;

use arc_swap::ArcSwap;
use caliban_provider::{Effort, Provider, ThinkingSetting, ToolChoice};

use crate::error::{Error, Result};
use crate::hooks::{Hooks, NoopHooks};
use crate::post_process::{AssistantPostProcessor, NoopPostProcessor};
use crate::registry::ToolRegistry;

/// Default per-turn parallel tool dispatch limit.
///
/// Returns `available_parallelism().get() - 1`, clamped to at least 1, so that
/// the agent loop, streaming, and the renderer can keep a core to themselves.
/// Falls back to 1 when `available_parallelism()` is unavailable.
///
/// # Panics
///
/// Cannot panic in practice: the value is clamped to `>= 1` via `.max(1)`
/// before being passed to `NonZeroUsize::new`.
#[must_use]
pub fn default_parallel_tool_limit() -> NonZeroUsize {
    let n = std::thread::available_parallelism()
        .map_or(2, NonZeroUsize::get)
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
    /// Extended-thinking control, swapped at runtime by `/think` (ticket
    /// #100). Lock-free reads via [`arc_swap::ArcSwap`], mirroring `effort`;
    /// the per-turn request builder snapshots it with `load_full()` and copies
    /// it into `CompletionRequest.thinking`. Default is
    /// [`ThinkingSetting::Auto`] (derive from `effort`, legacy behavior).
    pub thinking: Arc<ArcSwap<ThinkingSetting>>,
    /// Optional opaque user identifier forwarded to the provider.
    pub user_id: Option<String>,
    /// Maximum number of agentic turns before returning `Error::MaxTurnsReached`.
    pub max_turns: u32,
    /// Tool-choice policy sent to the provider.
    pub tool_choice: ToolChoice,
    // ── Plan A (turn-loop resilience) ────────────────────────────
    /// Stage A escalated `max_tokens` budget (used once per `MaxTokens` hit).
    pub escalated_max_tokens: u32,
    /// Stage B meta-continuation cap (per-run).
    pub max_meta_continuations: u8,
    /// Stream idle timeout (ms). 0 disables the watchdog. Governs the silence
    /// tolerated *after* the first output chunk (mid-content stall).
    pub stream_idle_timeout_ms: u32,
    /// Pre-first-token idle budget (ms) for the stream watchdog. Governs the
    /// silence tolerated *before* the first output chunk (slow local-model
    /// prefill on a large-context turn, #263). `0` falls back to
    /// `stream_idle_timeout_ms`. Frontier models prefill in ms and never
    /// approach this, so a single generous global default is safe.
    pub stream_prefill_timeout_ms: u32,
    /// Master switch for `MaxTokens` recovery (Stage A + B).
    pub max_tokens_recovery: bool,
    // ── Plan B (context-window management) ───────────────────────
    /// Pre-turn autocompaction threshold (utilization in 0..=1). `None` disables.
    pub auto_compact_threshold: Option<f32>,
    /// Enable the per-turn microcompact janitor pass.
    pub micro_compact_enabled: bool,
    /// Global per-tool-result cap in chars. `0` disables.
    pub tool_result_cap_chars: usize,
    /// Minimum estimated tokens on the last user message to merit a cache marker.
    pub min_cache_block_tokens: usize,
    // ── Plan C (TUI slash & UX polish) ───────────────────────────
    /// Reasoning-effort level, swapped at runtime by `/effort`. Lock-free
    /// reads via [`arc_swap::ArcSwap`]; the per-turn request builder
    /// snapshots this with `load_full()` and copies into
    /// `CompletionRequest.effort`. Default is [`Effort::Auto`].
    pub effort: Arc<ArcSwap<Effort>>,
    // ── ADR-0046 (two-stage tool surface) ────────────────────────
    /// When `true`, MCP tools are hidden from the wire payload until
    /// the model activates them via the `ToolSearch` built-in.
    /// Default `false` in v1.
    pub lazy_mcp: bool,
    /// Soft LRU cap on the activation set. `0` is treated as
    /// `lazy_mcp = false` by callers. Default `24`.
    pub max_active_schemas: usize,
    // ── #239 (no-edit-progress nudge) ────────────────────────────
    /// Number of consecutive turns with **zero** successful edit-class
    /// (non-[`crate::Tool::is_read_only`]) tool calls after which the loop
    /// injects a single neutral nudge encouraging the model to make the
    /// edit it has identified. At most one nudge fires per no-edit streak;
    /// the counter (and the nudge arming) resets the moment a non-read-only
    /// tool call succeeds. `0` disables the nudge entirely. Default `10`.
    pub no_edit_nudge_threshold: u32,
    // ── #249 (empty/degenerate-turn guard) ───────────────────────
    /// Maximum number of consecutive **degenerate** turns the loop will nudge
    /// before letting the run end. A degenerate turn is one that consumed
    /// output tokens yet produced no tool call and no actionable text — e.g.
    /// an Ollama reasoning model (gemma-family) that emits only a thinking
    /// block and then stops, which would otherwise end the run as a silent
    /// "success" with no work done. On such a turn the loop injects one neutral
    /// nudge and takes another turn; the streak counter resets the moment a
    /// productive turn occurs. `0` disables the guard entirely. Default `2`.
    pub empty_turn_nudge_max: u32,
    // ── #62 (runaway-thinking-spiral guard) ──────────────────────
    /// Per-turn cap on cumulative **thinking** characters. A reasoning model can
    /// stream thinking deltas continuously (so the idle watchdog never fires)
    /// without ever producing a tool call or final text; `max_tokens` recovery
    /// only *raises* the budget on each `MaxTokens` hit. When a single attempt
    /// streams more than this many thinking chars, the run terminates with
    /// [`crate::StopCondition::ThinkingBudgetExhausted`]. Independent of the
    /// idle watchdog and the output-token budget. `0` disables the guard.
    /// Default `262_144` (≈ 65k tokens) — a backstop far above any legitimate
    /// single-turn reasoning, so it never trips in normal use.
    pub max_turn_thinking_chars: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            stop_sequences: Vec::new(),
            thinking: Arc::new(ArcSwap::from_pointee(ThinkingSetting::Auto)),
            user_id: None,
            max_turns: 50,
            tool_choice: ToolChoice::default(),
            // Plan A
            escalated_max_tokens: 16_384,
            max_meta_continuations: 3,
            stream_idle_timeout_ms: 90_000,
            stream_prefill_timeout_ms: 300_000,
            // Stage A hoisted above TurnEnd yield + counter inc (PR #90+
            // follow-up); regression test
            // `stage_a_retry_does_not_double_count_turn` guards the
            // invariant. Default-on: opt out via CLI
            // `--max-tokens-recovery=false` or settings
            // `max_tokens_recovery = false`.
            max_tokens_recovery: true,
            // Plan B
            auto_compact_threshold: Some(0.75),
            micro_compact_enabled: true,
            tool_result_cap_chars: 50_000,
            min_cache_block_tokens: 1024,
            // Plan C
            effort: Arc::new(ArcSwap::from_pointee(Effort::Auto)),
            // ADR-0046
            lazy_mcp: false,
            max_active_schemas: 24,
            // #239
            no_edit_nudge_threshold: 10,
            // #249
            empty_turn_nudge_max: 2,
            // #62
            max_turn_thinking_chars: 262_144,
        }
    }
}

#[cfg(test)]
mod recovery_config_tests {
    use super::*;

    #[test]
    fn default_recovery_knobs() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.escalated_max_tokens, 16_384);
        assert_eq!(cfg.max_meta_continuations, 3);
        assert_eq!(cfg.stream_idle_timeout_ms, 90_000);
        assert_eq!(cfg.stream_prefill_timeout_ms, 300_000);
        // Stage A no longer double-counts the turn — recovery is
        // safely default-on. See stream/mod.rs Stage A hoist + the
        // stage_a_retry_does_not_double_count_turn regression test.
        assert!(cfg.max_tokens_recovery);
        assert_eq!(cfg.no_edit_nudge_threshold, 10);
        assert_eq!(cfg.empty_turn_nudge_max, 2);
        assert_eq!(cfg.max_turn_thinking_chars, 262_144);
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
    /// Runtime-swappable model id, seeded from `config.model` at
    /// construction time. Read via [`Agent::active_model`] (lock-free
    /// snapshot); swapped via [`Agent::try_swap_model`] (typically by
    /// the `/model` TUI command). The streaming loop reads this in three
    /// hot sites — instrument span, capabilities probe, request build —
    /// so a swap takes effect on the next turn without restart.
    pub(crate) active_model: Arc<ArcSwap<String>>,
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
    /// Shared plan-mode flag. When `Some` and the inner flag is set, the
    /// dispatcher rejects tools that are neither side-effect-free
    /// ([`crate::Tool::is_read_only`]) nor a plan-control tool
    /// ([`crate::plan_mode::is_plan_control_tool`]).
    /// `None` means plan-mode gating is disabled entirely.
    pub(crate) plan_mode: Option<crate::plan_mode::SharedPlanMode>,
    /// Post-processor applied to each assistant message's text blocks
    /// before the message is appended to the conversation history.
    /// Defaults to [`NoopPostProcessor`], which is a zero-cost identity.
    pub(crate) post_processor: Arc<dyn AssistantPostProcessor>,
    /// Sidecar activation set for lazy MCP tool loading (ADR-0046).
    /// Reads via `ArcSwap::load` are lock-free; writes go through
    /// `rcu` from `ToolSearch::invoke`. Snapshotted by
    /// `install_sub_agent` when frontmatter `inherit_active_mcp` is
    /// true (the default).
    pub(crate) mcp_active: Arc<ArcSwap<crate::mcp_activation::McpActivationSet>>,
    /// Names of MCP servers that opt out of lazy loading via
    /// `[server.X] lazy = false`. Tools belonging to these servers
    /// always ride the wire payload even when `config.lazy_mcp` is true.
    /// Resolved once at startup from `mcp.toml` / unified settings.
    pub(crate) mcp_eager_servers: Arc<std::collections::HashSet<String>>,
}

/// Error returned by [`Agent::try_swap_model`].
#[derive(Debug, Clone, thiserror::Error)]
pub enum ModelSwapError {
    /// The requested model id is not exposed by the active provider's
    /// `list_models()`. Heuristic: we accept the swap when the model is
    /// either in `list_models()` *or* the list is empty (mocks/tests).
    #[error("model `{0}` is not available on the active provider")]
    UnsupportedByProvider(String),
    /// The requested model would require a different provider than the
    /// one currently driving the Agent. Hot-swap across providers is
    /// deferred; surface a remediation hint instead.
    #[error(
        "model `{0}` requires provider `{1}`, but active provider is `{2}`; restart with --provider {1}"
    )]
    CrossProvider(String, String, String),
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

    /// Return a clone of the agent's hooks handle. Useful for binary code
    /// that needs to fire session/cwd/notification events from outside the
    /// turn loop.
    #[must_use]
    pub fn hooks(&self) -> Arc<dyn Hooks + Send + Sync> {
        Arc::clone(&self.hooks)
    }

    /// Return a clone of the agent's provider handle. Used by the TUI to
    /// resolve `Capabilities::max_input_tokens` for the context-window
    /// indicator (ADR 0033) without re-instantiating a provider client.
    #[must_use]
    pub fn provider(&self) -> Arc<dyn Provider + Send + Sync> {
        Arc::clone(&self.provider)
    }

    /// Return a clone of the agent's compactor handle. Used by the TUI's
    /// `/compact` slash command (ADR 0033) to manually trigger the
    /// configured compaction strategy.
    #[must_use]
    pub fn compactor(&self) -> Arc<dyn crate::compact::Compactor + Send + Sync> {
        Arc::clone(&self.compactor)
    }

    /// Return the current active model id as a lock-free `Arc<String>`
    /// snapshot. Reads inside the streaming loop go through this so a
    /// runtime `/model` swap takes effect on the next turn.
    #[must_use]
    pub fn active_model(&self) -> Arc<String> {
        self.active_model.load_full()
    }

    /// Return the shared MCP activation set handle (ADR-0046).
    /// Used by `ToolSearch::invoke` and `install_sub_agent`.
    #[must_use]
    pub fn mcp_active(&self) -> Arc<ArcSwap<crate::mcp_activation::McpActivationSet>> {
        Arc::clone(&self.mcp_active)
    }

    /// Return the eager-MCP-server set (ADR-0046). Sub-agent install
    /// shares this Arc with the child unchanged.
    #[must_use]
    pub fn mcp_eager_servers(&self) -> Arc<std::collections::HashSet<String>> {
        Arc::clone(&self.mcp_eager_servers)
    }

    /// Swap the active model in place (same-provider only). Returns
    /// [`ModelSwapError::UnsupportedByProvider`] if the model isn't
    /// recognised by the active provider's `list_models()` (when that
    /// list is non-empty). Mocks / test providers that return an empty
    /// model list accept any id, which keeps unit tests honest without
    /// requiring a fixture per provider.
    ///
    /// # Errors
    ///
    /// See [`ModelSwapError`]. Cross-provider swap is deferred and
    /// surfaces [`ModelSwapError::CrossProvider`] via the
    /// `/model` picker when the operator selects a non-selectable row.
    pub fn try_swap_model(&self, new_model: &str) -> std::result::Result<(), ModelSwapError> {
        let list = self.provider.list_models();
        let known = list
            .iter()
            .any(|m| m.id == new_model || m.native_id == new_model);
        if !list.is_empty() && !known {
            return Err(ModelSwapError::UnsupportedByProvider(new_model.to_string()));
        }
        self.active_model.store(Arc::new(new_model.to_string()));
        Ok(())
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
    plan_mode: Option<crate::plan_mode::SharedPlanMode>,
    post_processor: Option<Arc<dyn AssistantPostProcessor>>,
    mcp_active: Option<Arc<ArcSwap<crate::mcp_activation::McpActivationSet>>>,
    mcp_eager_servers: Option<Arc<std::collections::HashSet<String>>>,
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
            plan_mode: None,
            post_processor: None,
            mcp_active: None,
            mcp_eager_servers: None,
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

    /// Toggle the two-stage `MaxTokens` recovery flow. Default `true`.
    /// Stage A silently retries with `escalated_max_tokens`; Stage B
    /// injects a meta-continuation prompt and advances a turn.
    #[must_use]
    pub fn max_tokens_recovery(mut self, on: bool) -> Self {
        self.config.max_tokens_recovery = on;
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

    /// Attach a shared plan-mode flag. When set and the inner flag is `true`,
    /// the dispatcher rejects tools that are neither side-effect-free
    /// ([`crate::Tool::is_read_only`]) nor a plan-control tool. Default: `None`
    /// (gating off).
    #[must_use]
    pub fn plan_mode(mut self, handle: crate::plan_mode::SharedPlanMode) -> Self {
        self.plan_mode = Some(handle);
        self
    }

    /// Install a post-processor that mutates the text of each assistant
    /// message before it is appended to the conversation history. Defaults
    /// to [`NoopPostProcessor`] (identity).
    ///
    /// The canonical use today is the `Learning` output style, which
    /// inserts `TODO(human)` markers at function-definition inflection
    /// points.
    #[must_use]
    pub fn post_processor(mut self, p: Arc<dyn AssistantPostProcessor>) -> Self {
        self.post_processor = Some(p);
        self
    }

    /// Set the shared MCP activation set (ADR-0046). Default: a fresh
    /// set sized by `config.max_active_schemas`. Supply an externally
    /// constructed Arc when `ToolSearch` and the `Agent` need to share
    /// the same activation surface.
    #[must_use]
    pub fn mcp_active(
        mut self,
        active: Arc<ArcSwap<crate::mcp_activation::McpActivationSet>>,
    ) -> Self {
        self.mcp_active = Some(active);
        self
    }

    /// Set the eager-MCP-server set (ADR-0046). Default: empty.
    #[must_use]
    pub fn mcp_eager_servers(mut self, servers: Arc<std::collections::HashSet<String>>) -> Self {
        self.mcp_eager_servers = Some(servers);
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
        let active_model = Arc::new(ArcSwap::from_pointee(self.config.model.clone()));
        let mcp_active = self.mcp_active.unwrap_or_else(|| {
            Arc::new(ArcSwap::from_pointee(
                crate::mcp_activation::McpActivationSet::new(self.config.max_active_schemas),
            ))
        });
        let mcp_eager_servers = self
            .mcp_eager_servers
            .unwrap_or_else(|| Arc::new(std::collections::HashSet::new()));
        Ok(Agent {
            provider,
            tools: self.tools,
            config: self.config,
            active_model,
            compactor: self
                .compactor
                .unwrap_or_else(|| Arc::new(crate::compact::NoopCompactor)),
            retry: self.retry.unwrap_or_default(),
            hooks: self.hooks.unwrap_or_else(|| Arc::new(NoopHooks)),
            prompt_cache: self.prompt_cache,
            parallel_tools: self.parallel_tools,
            parallel_tool_limit: self.parallel_tool_limit,
            plan_mode: self.plan_mode,
            post_processor: self
                .post_processor
                .unwrap_or_else(|| Arc::new(NoopPostProcessor)),
            mcp_active,
            mcp_eager_servers,
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
        let cores = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);
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

    #[test]
    fn builder_defaults_post_processor_to_none_until_built() {
        let b = AgentBuilder::default();
        assert!(
            b.post_processor.is_none(),
            "builder field is None until build() defaults it to NoopPostProcessor",
        );
    }

    #[test]
    fn builder_post_processor_setter_accepts_arc_trait_object() {
        let pp: Arc<dyn AssistantPostProcessor> = Arc::new(NoopPostProcessor);
        let b = AgentBuilder::default().post_processor(pp);
        assert!(b.post_processor.is_some());
    }
}

#[cfg(test)]
mod context_config_tests {
    use super::AgentConfig;

    #[test]
    fn default_context_knobs() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.auto_compact_threshold, Some(0.75));
        assert!(cfg.micro_compact_enabled);
        assert_eq!(cfg.tool_result_cap_chars, 50_000);
        assert_eq!(cfg.min_cache_block_tokens, 1024);
    }
}

#[cfg(test)]
mod effort_tests {
    use super::*;

    #[test]
    fn openai_mapping() {
        assert_eq!(Effort::Low.as_openai(), Some("low"));
        assert_eq!(Effort::Medium.as_openai(), Some("medium"));
        assert_eq!(Effort::High.as_openai(), Some("high"));
        assert_eq!(Effort::Max.as_openai(), Some("high"));
        assert_eq!(Effort::Auto.as_openai(), None);
    }

    #[test]
    fn anthropic_budget_mapping() {
        assert_eq!(Effort::Low.as_anthropic_budget(), Some(2_048));
        assert_eq!(Effort::Medium.as_anthropic_budget(), Some(8_192));
        assert_eq!(Effort::High.as_anthropic_budget(), Some(24_576));
        assert_eq!(Effort::Max.as_anthropic_budget(), Some(64_000));
        assert_eq!(Effort::Auto.as_anthropic_budget(), None);
    }

    #[test]
    fn config_default_effort_is_auto() {
        let cfg = AgentConfig::default();
        assert_eq!(*cfg.effort.load_full(), Effort::Auto);
    }

    #[test]
    fn config_default_thinking_is_auto() {
        let cfg = AgentConfig::default();
        assert_eq!(*cfg.thinking.load_full(), ThinkingSetting::Auto);
    }
}
