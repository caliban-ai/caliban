//! Composition-root helpers for the `caliban` binary.
//!
//! Hosts every helper that constructs runtime state shared across the
//! three dispatch paths (TUI / headless / single-prompt):
//!
//! - [`init_debug_tracing`] — file-backed `tracing` subscriber.
//! - [`build_provider`] — single-provider construction (router fallback).
//! - [`web_fetch_client`] — `reqwest::Client` for `WebFetchTool`.
//! - [`build_registry`] — registry assembly (built-in tools + plugins).
//! - [`load_layered_settings`] — ADR 0026 `settings.json` loader.
//! - [`auto_memory_disabled`] — `CALIBAN_DISABLE_AUTO_MEMORY` check.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use caliban_agent_core::{Agent, ToolRegistry};
use caliban_provider::{Provider, Usage};
use caliban_sessions::{PersistedSession, SessionStore};
use caliban_skills::{SkillTool, load_skills_report, register_builtins};
use caliban_tools_builtin::{
    AgentFactory, AgentTool, AgentToolInput, BashOutputTool, BashTool, EditTool, EnterPlanModeTool,
    ExitPlanModeTool, GlobTool, GrepTool, KillShellTool, MultiEditTool, NotebookEditTool,
    ReadMemoryTopicTool, ReadTool, TodoWriteTool, WebFetchTool, WebSearchTool, WorkspaceRoot,
    WriteMemoryTopicTool, WriteTool,
};

use crate::agents_cli;
use crate::args::{Args, ProviderKind, provider_name, resolved_provider};
use crate::provider_wiring::{resolve_key, wrap_with_refresh_if_helper};
use crate::{headless, system_prompt, tui};

/// Returns `true` when file-backed debug logging should be installed:
/// `--debug`, an explicit `--debug-file`/`CALIBAN_DEBUG_FILE`, or
/// `CALIBAN_DEBUG` in the environment. Naming a destination implies
/// debug-on, so `--debug-file` alone is enough.
fn debug_enabled(args: &Args) -> bool {
    args.debug || args.debug_file.is_some() || std::env::var("CALIBAN_DEBUG").is_ok()
}

/// Resolve the destination for the debug log, or `None` when debug logging
/// is disabled. An explicit `--debug-file` path wins verbatim (relative
/// paths resolve against CWD at open time); otherwise the default
/// `<cache_dir>/caliban/debug.log` is used.
fn resolve_debug_log_path(args: &Args) -> Option<std::path::PathBuf> {
    if !debug_enabled(args) {
        return None;
    }
    if let Some(path) = args.debug_file.clone() {
        return Some(path);
    }
    dirs::cache_dir().map(|d| d.join("caliban").join("debug.log"))
}

/// Install a file-backed `tracing` subscriber when debug logging is enabled
/// (see [`debug_enabled`]). No-op otherwise. Idempotent once initialized:
/// runs at most once at startup before any `tracing::*!` site fires.
pub(crate) async fn init_debug_tracing(args: &Args) {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let Some(log_path) = resolve_debug_log_path(args) else {
        return;
    };
    if let Some(parent) = log_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let opened = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await;
    let Ok(async_file) = opened else { return };
    // tracing-subscriber's fmt layer wants std::io::Write, so
    // convert back to a std::fs::File. into_std offloads to the
    // blocking pool; safe here since this only runs once at start.
    let file = async_file.into_std().await;
    // Default filter keeps caliban + caliban_* crates at DEBUG and
    // silences noisy lower-level traces (mio, hyper, reqwest, …).
    // Users can override via RUST_LOG env var.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "debug,mio=warn,hyper=warn,hyper_util=warn,reqwest=warn,h2=warn,rustls=warn,tower=warn",
        )
    });
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false);
    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .init();
    tracing::info!("caliban debug logging started — {}", log_path.display());
}

pub(crate) fn build_provider(
    args: &Args,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<Arc<dyn Provider + Send + Sync>> {
    use ProviderKind::{Anthropic, Google, Ollama, Openai};
    Ok(match resolved_provider(args) {
        Anthropic => build_anthropic(pool)?,
        Openai => build_openai(pool)?,
        Ollama => {
            use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
            // `from_env` already returns the local default when
            // `OLLAMA_BASE_URL` is unset. Only the case where the env var is
            // set but unparseable yields `Err`, and that should reach the
            // operator instead of silently retargeting localhost.
            Arc::new(OllamaProvider::direct(
                DirectConfig::from_env().context("invalid OLLAMA_BASE_URL")?,
            )?)
        }
        Google => build_google(pool)?,
    })
}

fn build_anthropic(
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<Arc<dyn Provider + Send + Sync>> {
    use caliban_provider_anthropic::error::AnthropicError;
    use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
    use secrecy::SecretString;

    let provider_id = "anthropic";
    // Capture env-driven overrides once so they survive a rebuild.
    let base_url = std::env::var("ANTHROPIC_BASE_URL").ok();
    let version = std::env::var("ANTHROPIC_VERSION").ok();

    let make_cfg = move |key: SecretString| -> Result<DirectConfig> {
        let mut cfg = DirectConfig::new(key);
        if let Some(url) = base_url.as_deref() {
            cfg.base_url = url::Url::parse(url)
                .with_context(|| format!("invalid ANTHROPIC_BASE_URL {url:?}"))?;
        }
        if let Some(v) = version.as_deref() {
            cfg.anthropic_version = v.to_string();
        }
        Ok(cfg)
    };

    if pool.has_spec_for(provider_id) {
        let key = resolve_key(provider_id, "ANTHROPIC_API_KEY", pool)?;
        let inner = AnthropicProvider::direct(make_cfg(key)?)?;
        let make_cfg2 = make_cfg.clone();
        let rebuild = move |k: SecretString| -> std::result::Result<_, caliban_provider::Error> {
            let cfg = make_cfg2(k).map_err(|e| caliban_provider::Error::Adapter(e.into()))?;
            AnthropicProvider::direct(cfg).map_err(caliban_provider::Error::adapter)
        };
        Ok(wrap_with_refresh_if_helper(
            inner,
            pool,
            provider_id,
            "anthropic",
            rebuild,
        ))
    } else {
        // Env-only path (preserves the existing F2 diagnostics).
        let cfg = DirectConfig::from_env().map_err(|e| match e {
            AnthropicError::MissingConfig(name) => missing_key_err(name),
            AnthropicError::Transport(inner) => {
                anyhow!("invalid ANTHROPIC_BASE_URL: {inner} — unset it or supply a valid URL")
            }
            other => anyhow!(other),
        })?;
        Ok(Arc::new(AnthropicProvider::direct(cfg)?))
    }
}

fn build_openai(
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<Arc<dyn Provider + Send + Sync>> {
    use caliban_provider_openai::error::OpenAIError;
    use caliban_provider_openai::{OpenAIProvider, config::DirectConfig};
    use secrecy::SecretString;

    let provider_id = "openai";
    let base_url = std::env::var("OPENAI_BASE_URL").ok();
    let organization = std::env::var("OPENAI_ORG_ID").ok();
    let project = std::env::var("OPENAI_PROJECT").ok();

    let make_cfg = move |key: SecretString| -> Result<DirectConfig> {
        DirectConfig::from_parts(
            key,
            base_url.as_deref(),
            organization.clone(),
            project.clone(),
        )
        .map_err(|e| match e {
            OpenAIError::InvalidBaseUrl { value, source } => {
                anyhow!("invalid OPENAI_BASE_URL {value:?}: {source}")
            }
            other => anyhow!(other),
        })
    };

    if pool.has_spec_for(provider_id) {
        let key = resolve_key(provider_id, "OPENAI_API_KEY", pool)?;
        let inner = OpenAIProvider::direct(make_cfg(key)?)?;
        let make_cfg2 = make_cfg.clone();
        let rebuild = move |k: SecretString| -> std::result::Result<_, caliban_provider::Error> {
            let cfg = make_cfg2(k).map_err(|e| caliban_provider::Error::Adapter(e.into()))?;
            OpenAIProvider::direct(cfg).map_err(caliban_provider::Error::adapter)
        };
        Ok(wrap_with_refresh_if_helper(
            inner,
            pool,
            provider_id,
            "openai",
            rebuild,
        ))
    } else {
        let cfg = DirectConfig::from_env().map_err(|e| match e {
            OpenAIError::MissingConfig(name) => missing_key_err(name.as_str()),
            OpenAIError::InvalidBaseUrl { value, source } => anyhow!(
                "invalid OPENAI_BASE_URL {value:?}: {source} — unset it or supply a valid URL"
            ),
            other => anyhow!(other),
        })?;
        Ok(Arc::new(OpenAIProvider::direct(cfg)?))
    }
}

fn build_google(
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<Arc<dyn Provider + Send + Sync>> {
    use caliban_provider_google::error::GoogleError;
    use caliban_provider_google::{GoogleProvider, config::AIStudioConfig};
    use secrecy::SecretString;

    let provider_id = "google";

    if pool.has_spec_for(provider_id) {
        let key = resolve_key(provider_id, "GEMINI_API_KEY", pool)?;
        let inner = GoogleProvider::ai_studio(AIStudioConfig::new(key))?;
        let rebuild = move |k: SecretString| -> std::result::Result<_, caliban_provider::Error> {
            GoogleProvider::ai_studio(AIStudioConfig::new(k))
                .map_err(caliban_provider::Error::adapter)
        };
        Ok(wrap_with_refresh_if_helper(
            inner,
            pool,
            provider_id,
            "google",
            rebuild,
        ))
    } else {
        let cfg = AIStudioConfig::from_env().map_err(|e| match e {
            GoogleError::MissingConfig(name) => missing_key_err(name),
            GoogleError::Transport(inner) => {
                anyhow!("invalid GEMINI_BASE_URL: {inner} — unset it or supply a valid URL")
            }
            other => anyhow!(other),
        })?;
        Ok(Arc::new(GoogleProvider::ai_studio(cfg)?))
    }
}

/// Format the canonical "API key is missing" surface line. Centralized so
/// every provider arm of [`build_provider`] uses the same wording, and so
/// the URL-parse error paths can clearly *not* trigger it (F2).
fn missing_key_err(env_var: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{env_var} is not set — export it, configure `apiKeyHelper` in \
         settings.json (ADR 0026), or pick a different `--provider`"
    )
}

/// Pre-flight check: when targeting a non-canonical `OpenAI` endpoint
/// (LM Studio, vLLM, `llama.cpp-server`, …), confirm the requested model
/// is loaded before the agent loop fires its first request. Local servers
/// like LM Studio silently substitute the first-loaded model for unknown
/// model IDs and return a normal HTTP 200, so the typo never surfaces as
/// an error and a wrong model runs the whole session (F4 from the 2026-
/// 05-27 lmstudio probe).
///
/// Skipped for:
/// - Non-`OpenAI` providers (Anthropic / Google / Ollama have their own
///   handling; Anthropic + Google 404 unknown IDs cleanly).
/// - Canonical `api.openai.com` (already 404s on unknown IDs).
/// - When `OPENAI_BASE_URL` is unset (defaults to api.openai.com — same).
/// - Network errors on the listing (treat as informational warning;
///   the actual request will surface a more specific error).
pub(crate) async fn preflight_model_check(args: &Args, model: &str) -> Result<()> {
    if !matches!(resolved_provider(args), ProviderKind::Openai) {
        return Ok(());
    }
    let Ok(base) = std::env::var("OPENAI_BASE_URL") else {
        return Ok(());
    };
    let Ok(parsed) = url::Url::parse(&base) else {
        // The unparseable-URL path is handled by build_provider; don't
        // double-surface here.
        return Ok(());
    };
    // Skip canonical OpenAI — public catalog is too dynamic to enumerate
    // reliably, and the wire-level 404 already produces a clean error.
    if matches!(parsed.host_str(), Some(h) if h.ends_with("openai.com")) {
        return Ok(());
    }

    let mut models_url = parsed.clone();
    {
        let path = models_url.path().trim_end_matches('/').to_string();
        models_url.set_path(&format!("{path}/models"));
    }

    // Don't escalate http-client construction failures here; the
    // agent's own request will surface them.
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    else {
        return Ok(());
    };
    let mut req = client.get(models_url);
    if let Ok(k) = std::env::var("OPENAI_API_KEY") {
        req = req.bearer_auth(k);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        // Reachability errors are not the operator's fault here — fall
        // through and let the agent loop surface the real network error
        // with its full context. Just print a hint on stderr so the
        // pre-flight isn't completely invisible.
        Err(e) => {
            eprintln!(
                "[caliban] note: model pre-flight could not reach {base} ({e}); proceeding with request"
            );
            return Ok(());
        }
    };
    if !resp.status().is_success() {
        // Non-2xx: same logic — the agent's request will explain it
        // better (auth, 5xx, etc.).
        return Ok(());
    }
    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };
    let models: Vec<String> = body
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if models.is_empty() {
        // Some local servers list nothing until a model is loaded; in
        // that case we can't usefully compare. Let the request surface
        // the real "no models loaded" error.
        return Ok(());
    }
    if models.iter().any(|m| m == model) {
        return Ok(());
    }
    let listed = models.join(", ");
    Err(anyhow::anyhow!(
        "model {model:?} is not loaded at OPENAI_BASE_URL={base}; loaded models: {listed} \
         (pass --model with one of those names; LM Studio and similar servers silently \
         substitute the first loaded model for unknown IDs, so this check fails fast)"
    ))
}

pub(crate) fn build_registry(
    args: &Args,
    workspace: WorkspaceRoot,
    todos: caliban_agent_core::SharedTodos,
    plan_mode: caliban_agent_core::SharedPlanMode,
    plugin_skill_roots: &[PathBuf],
) -> ToolRegistry {
    if args.no_tools {
        return ToolRegistry::new();
    }
    let workspace_root = workspace.root().to_path_buf();
    let root = if args.restrict_paths {
        workspace.restricted()
    } else {
        workspace
    };
    let mut r = ToolRegistry::new();
    r.register(Arc::new(ReadTool::new(root.clone())));
    r.register(Arc::new(WriteTool::new(root.clone())));
    r.register(Arc::new(EditTool::new(root.clone())));
    r.register(Arc::new(MultiEditTool::new(root.clone())));
    r.register(Arc::new(NotebookEditTool::new(root.clone())));
    r.register(Arc::new(BashTool::new(root.clone())));
    r.register(Arc::new(GlobTool::new(root.clone())));
    r.register(Arc::new(GrepTool::new(root)));
    r.register(Arc::new(WebFetchTool::new(web_fetch_client())));
    r.register(Arc::new(WebSearchTool::new(web_fetch_client())));
    r.register(Arc::new(BashOutputTool::with_global_registry()));
    r.register(Arc::new(KillShellTool::with_global_registry()));
    r.register(Arc::new(TodoWriteTool::new(todos)));
    r.register(Arc::new(EnterPlanModeTool::new(Arc::clone(&plan_mode))));
    r.register(Arc::new(ExitPlanModeTool::new(plan_mode)));
    // Auto-memory tools — kill switch via env per ADR 0035. The skill body
    // documents how to use the tools; without the skill, the model has no
    // protocol manual, so we gate both together. Skipped in bare mode.
    if !auto_memory_disabled() && !args.bare {
        let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);
        let topic_loader = Arc::new(caliban_memory::TopicLoader::new(cfg.auto_memory_dir));
        r.register(Arc::new(ReadMemoryTopicTool::new(Arc::clone(
            &topic_loader,
        ))));
        r.register(Arc::new(WriteMemoryTopicTool::new(topic_loader)));
    }

    if !args.no_skills && !args.bare {
        let mut roots = caliban_skills::default_roots(&workspace_root);
        roots.extend(plugin_skill_roots.iter().cloned());
        let report = load_skills_report(&roots);
        // A discovered-but-rejected skill (name/dir mismatch, bad frontmatter)
        // would otherwise vanish into a trace-level log. Warn on stderr so it
        // is user-visible in CLI/headless runs; `caliban doctor` mirrors this
        // for the TUI. See issue #107.
        for skip in &report.skips {
            eprintln!(
                "caliban: warning: skipping skill {} — {}",
                skip.path.display(),
                skip.reason
            );
        }
        let mut skills = report.skills;
        // Built-in skills register *before* user-dir scan results win — except
        // that the loader already shadows duplicates, so `register_builtins`
        // is a no-op if the user shipped their own `auto-memory` skill.
        // We hide the built-in entirely when the kill switch is set, matching
        // the tool gating above.
        if !auto_memory_disabled() {
            register_builtins(&mut skills);
        }
        r.register(Arc::new(SkillTool::new(skills)));
    }
    r
}

/// Drive the layered `settings.json` loader (ADR 0026).
///
/// Honors `--bare`, `--settings`, and `--setting-sources`. When the
/// unified file is absent, legacy `permissions.toml`, `mcp.toml`, and
/// `hooks.toml` paths still load via the existing per-feature loaders
/// (handled in their respective wire-up sites below).
pub(crate) fn load_layered_settings(
    args: &Args,
    workspace_root: &std::path::Path,
) -> Result<caliban_settings::LoadOutcome> {
    let mut opts = caliban_settings::LoadOptions::new(workspace_root.to_path_buf());
    opts.bare = args.bare;
    if let Some(csv) = args.setting_sources.as_deref() {
        opts = opts.with_sources_csv(csv).map_err(|e| anyhow::anyhow!(e))?;
    }
    if let Some(overlay) = args.settings_overlay.as_deref() {
        opts = opts
            .with_cli_overlay(overlay)
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    let outcome = caliban_settings::load_settings(&opts).map_err(|e| anyhow::anyhow!(e))?;
    Ok(outcome)
}

/// Returns true if the user has opted out of the auto-memory feature.
/// Matches the loader-side check in `caliban_memory::loader`.
pub(crate) fn auto_memory_disabled() -> bool {
    matches!(
        std::env::var("CALIBAN_DISABLE_AUTO_MEMORY").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "True" | "yes")
    )
}

/// Build the shared `reqwest::Client` used by [`WebFetchTool`].
///
/// Manual redirect handling is required (the tool applies its own same-host
/// policy and surfaces cross-host redirects), so `Policy::none()` is set
/// here. A separate client is intentional — provider transports configure
/// their own clients and have different timeout/keep-alive needs.
pub(crate) fn web_fetch_client() -> reqwest::Client {
    caliban_common::http::no_redirect_client()
}

/// Discover plugins early (ADR 0030). Plugins contribute skill roots,
/// hooks config, MCP servers, agents, and output styles, so the manager
/// is constructed before any of those subsystems init. `--bare` and the
/// `--no-plugins` kill switch both produce an empty manager.
pub(crate) fn load_plugin_manager(
    args: &Args,
    workspace_root: &std::path::Path,
) -> caliban_plugins::PluginManager {
    if args.bare || args.no_plugins {
        return caliban_plugins::PluginManager::default();
    }
    let ws_for_plugins = args
        .workspace
        .clone()
        .unwrap_or_else(|| workspace_root.to_path_buf());
    let roots = caliban_plugins::PluginRoots::default_for(&ws_for_plugins);
    let settings = caliban_plugins::PluginSettings::from_env();
    match caliban_plugins::PluginManager::load(&roots, &settings) {
        Ok(mgr) => {
            if !mgr.loaded().is_empty() {
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_PLUGINS,
                    count = mgr.loaded().len(),
                    "loaded plugins",
                );
            }
            for f in mgr.failures() {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_PLUGINS,
                    path = %f.root_dir.display(),
                    error = %f.error,
                    "plugin failed to load",
                );
            }
            mgr
        }
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_PLUGINS, error = %e, "plugin discovery failed; continuing without plugins");
            caliban_plugins::PluginManager::default()
        }
    }
}

/// MCP servers — Phase B: stdio + HTTP + SSE transports (ADR 0023).
/// `--bare` (ADR 0025) suppresses MCP discovery entirely for reproducible CI.
/// Returns `(summaries, server_cfg)` — the latter is retained so the
/// permissions setup downstream can fold `[server.X.permissions]` blocks
/// into the global rule list.
///
/// Reads the MCP server map from the unified `Settings` snapshot (ADR 0026);
/// the legacy `caliban_mcp_client::load_config` loader is reachable through
/// the `caliban-settings` compat shim during the one-release deprecation
/// window and is no longer called directly from the binary.
pub(crate) async fn start_mcp(
    args: &Args,
    settings_snapshot: &caliban_settings::Settings,
    registry: &mut ToolRegistry,
) -> (
    Vec<caliban_mcp_client::ServerSummary>,
    std::collections::BTreeMap<String, caliban_mcp_client::ServerConfig>,
    Vec<caliban_agent_core::mcp_activation::McpToolInfo>,
) {
    if args.no_mcp || args.bare {
        return (Vec::new(), std::collections::BTreeMap::new(), Vec::new());
    }
    let cfg = settings_snapshot.mcp_config();
    let servers_for_perms = cfg.servers.clone();
    match caliban_mcp_client::McpClientManager::start(&cfg).await {
        Ok(mgr) => {
            // Snapshot the MCP tool directory for ToolSearch (ADR-0046)
            // BEFORE register_all consumes the manager state.
            let mcp_tools_for_search = mgr.list_mcp_tools();
            mgr.register_all(registry);
            if mgr.enabled_count() > 0 || mgr.skipped_disabled() > 0 || mgr.failed_count() > 0 {
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_MCP,
                    connected = mgr.enabled_count(),
                    failed = mgr.failed_count(),
                    disabled = mgr.skipped_disabled(),
                    "mcp manager started",
                );
            }
            (
                mgr.summaries().to_vec(),
                servers_for_perms,
                mcp_tools_for_search,
            )
        }
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_MCP, error = %e, "mcp manager start failed; continuing without MCP");
            (Vec::new(), servers_for_perms, Vec::new())
        }
    }
}

/// Compute the eager-MCP-server set from a server config map (ADR-0046).
/// A server is eager when `[mcp_servers.X] lazy = false`.
pub(crate) fn compute_mcp_eager_servers(
    server_cfg: &std::collections::BTreeMap<String, caliban_mcp_client::ServerConfig>,
) -> std::collections::HashSet<String> {
    server_cfg
        .iter()
        .filter(|(_, cfg)| cfg.lazy == Some(false))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Register the `ToolSearch` built-in into `registry` (ADR-0046).
///
/// `mcp_tools` is a snapshot taken at MCP startup time; the closure
/// captures it so `ToolSearch` can enumerate without holding the
/// manager. `mcp_active` is the shared activation set — the same Arc
/// must be threaded into the Agent so subsequent turns see the model's
/// activations.
///
/// Skipped when `--no-tools` is set or when MCP loading is disabled
/// entirely. In the latter case `ToolSearch` could still be registered
/// (it gracefully no-ops with "No MCP servers configured"), but
/// dropping it keeps the wire palette one entry leaner.
pub(crate) fn install_tool_search(
    args: &Args,
    registry: &mut ToolRegistry,
    mcp_tools: Vec<caliban_agent_core::mcp_activation::McpToolInfo>,
    mcp_active: Arc<arc_swap::ArcSwap<caliban_agent_core::mcp_activation::McpActivationSet>>,
) {
    if args.no_tools || args.no_mcp || args.bare {
        return;
    }
    let directory: caliban_tools_builtin::tool_search::DirectoryFn =
        Arc::new(move || mcp_tools.clone());
    let tool = caliban_tools_builtin::tool_search::ToolSearchTool::new(directory, mcp_active);
    registry.register(Arc::new(tool));
}

/// Wire `AgentTool` (the sub-agent primitive) into `registry`.
///
/// The factory closes over a snapshot of the parent registry (which DOES
/// NOT include `AgentTool`, so sub-agents cannot recurse) + the parent's
/// provider + chosen model. Hook inheritance is deferred to v2 — sub-agents
/// currently use `NoopHooks`. The background-handoff spawner asks the
/// per-repo supervisor daemon (auto-spawned if needed) to register a new
/// agent and return its socket (ADR 0037).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn install_sub_agent(
    args: &Args,
    registry: &mut ToolRegistry,
    provider: &Arc<dyn Provider + Send + Sync>,
    model: &str,
    parent_mcp_active: Arc<arc_swap::ArcSwap<caliban_agent_core::mcp_activation::McpActivationSet>>,
    parent_mcp_eager: Arc<std::collections::HashSet<String>>,
    parent_max_active_schemas: usize,
    parent_lazy_mcp: bool,
    inheritable_config: Option<crate::hook_inherit::InheritableHookConfig>,
    parent_runtime_rules: Arc<caliban_agent_core::RuntimeRuleStore>,
) {
    if args.no_sub_agent || args.no_tools {
        return;
    }
    let snapshot_names: Vec<String> = registry.names().map(str::to_string).collect();
    let mut snapshot = ToolRegistry::new();
    for name in &snapshot_names {
        if let Some(t) = registry.get(name) {
            snapshot.register(Arc::clone(t));
        }
    }
    let provider_for_factory: Arc<dyn Provider + Send + Sync> = Arc::clone(provider);
    let parent_model = model.to_string();
    let parent_max_tokens = args.max_tokens;
    let factory: AgentFactory = Arc::new(move |input: &AgentToolInput| {
        let chosen_model = input.model.clone().unwrap_or_else(|| parent_model.clone());
        let child_registry = match &input.tool_allowlist {
            Some(names) => {
                let mut r = ToolRegistry::new();
                for n in names {
                    if n == "AgentTool" {
                        continue;
                    }
                    if let Some(t) = snapshot.get(n) {
                        r.register(Arc::clone(t));
                    }
                }
                r
            }
            None => snapshot.clone(),
        };
        // ADR-0046: snapshot the parent's MCP activation set when the
        // frontmatter opts in (default true). The shared eager-server
        // set is always inherited because it reflects configuration
        // (per-server lazy = false), not session state.
        let child_active_set = if input.inherit_active_mcp {
            parent_mcp_active.load().snapshot()
        } else {
            caliban_agent_core::mcp_activation::McpActivationSet::new(parent_max_active_schemas)
        };
        let child_active = Arc::new(arc_swap::ArcSwap::from_pointee(child_active_set));
        let cfg = caliban_agent_core::AgentConfig {
            model: chosen_model,
            max_tokens: parent_max_tokens,
            max_turns: 20,
            lazy_mcp: parent_lazy_mcp,
            max_active_schemas: parent_max_active_schemas,
            ..caliban_agent_core::AgentConfig::default()
        };
        Agent::builder()
            .provider(Arc::clone(&provider_for_factory))
            .tools(child_registry)
            .config(cfg)
            .mcp_active(child_active)
            .mcp_eager_servers(Arc::clone(&parent_mcp_eager))
            .build()
            .expect("sub-agent builder")
    });
    // Background-handoff spawner (ADR 0037). When the parent invokes
    // AgentTool with `background: true`, the tool calls this closure;
    // we ask the per-repo supervisor daemon (auto-spawned if needed)
    // to register a new agent and return its socket. Closure-based
    // hooks are dropped at the boundary — the parent's `Hooks` chain
    // can't cross processes; see ADR 0037 ("Hook inheritance").
    let cwd_for_bg = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let repo_for_bg = agents_cli::discover_repo_root(&cwd_for_bg);
    // Clone once outside the Fn closure so the closure can call .as_ref()
    // on each invocation without consuming the captured value (#84).
    let inheritable_config_for_bg = inheritable_config;
    // The parent's live runtime-rule store, snapshotted per spawn so an
    // "Always allow/deny" the operator set after startup still reaches the
    // child (the config template captured above carries an empty list). (#114)
    let runtime_rules_for_bg = parent_runtime_rules;
    // Compute the parent's provider name once so background sub-agents
    // inherit the same provider by default (#93).
    let parent_provider = crate::provider_name(crate::resolved_provider(args)).to_string();
    let bg_spawner: caliban_tools_builtin::BackgroundSpawner = {
        let repo = repo_for_bg.clone();
        Arc::new(move |input: &AgentToolInput| {
            // Use a blocking handle to the current runtime so the
            // AgentTool stays sync-callable from its async invoke.
            let rt = tokio::runtime::Handle::current();
            let spec = caliban_supervisor::SpawnSpec {
                label: input.label.clone(),
                frontmatter_path: None,
                initial_prompt: input.prompt.clone(),
                model: input.model.clone(),
                provider: Some(parent_provider.clone()),
                tool_allowlist: input.tool_allowlist.clone(),
                isolation_worktree: matches!(
                    input.isolation,
                    caliban_tools_builtin::IsolationMode::Worktree
                ),
                inherit_hooks: input.inherit_hooks,
                interactive: false,
                inherited_hooks_config: if input.inherit_hooks {
                    inheritable_config_for_bg.as_ref().and_then(|cfg| {
                        // Snapshot the parent's LIVE runtime rules at spawn time
                        // and stamp them into a per-spawn clone of the template
                        // before serializing, so the child enforces them. (#114)
                        let mut cfg = cfg.clone();
                        cfg.runtime_rules = runtime_rules_for_bg.snapshot();
                        cfg.to_json()
                    })
                } else {
                    None
                },
            };
            let repo = repo.clone();
            // We can't `await` directly inside a non-async closure;
            // block on a fresh task instead.
            let (id, socket_path) = rt
                .block_on(async move {
                    let client = agents_cli::ensure_daemon_for_repo(&repo).await?;
                    client.spawn(spec).await.map_err(anyhow::Error::from)
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "background spawn failed");
                    (
                        format!("err-{}", uuid::Uuid::new_v4().simple()),
                        std::path::PathBuf::from("/dev/null"),
                    )
                });
            caliban_tools_builtin::BackgroundSpawnResult { id, socket_path }
        })
    };
    registry.register(Arc::new(
        AgentTool::new(factory, None).with_background_spawner(bg_spawner),
    ));
}

/// Project the hooks configuration out of the layered `Settings` snapshot.
/// Empty config when `--no-hooks` is set or `--bare` is in effect.
///
/// Reads from `Settings::hook_config()` (ADR 0026) instead of the legacy
/// `caliban_agent_core::HooksConfig::load` loader; the latter is now
/// `#[deprecated]` and remains reachable only through the
/// `caliban-settings::compat` shim during the one-release back-compat
/// window.
pub(crate) fn load_hooks_config(
    args: &Args,
    settings_snapshot: &caliban_settings::Settings,
) -> caliban_agent_core::HooksConfig {
    if args.no_hooks || args.bare {
        return caliban_agent_core::HooksConfig::default();
    }
    settings_snapshot.hook_config()
}

/// Outcome of [`build_permissions`]: the `Hooks` layer (or `None` when
/// `--no-permissions` is set), the `Ask`-bridge receiver for TUI mode,
/// and the auto-mode classifier (cloned later into the TUI driver).
pub(crate) struct PermissionsSetup {
    pub permissions_hook: Option<Arc<dyn caliban_agent_core::Hooks + Send + Sync>>,
    pub tui_ask_rx: Option<tokio::sync::mpsc::UnboundedReceiver<tui::ask::AskRequest>>,
    pub auto_mode_classifier: Option<Arc<caliban_agent_core::AutoModeClassifier>>,
    /// Shared session-scoped runtime-rule store. The same `Arc` is held by
    /// the permission gate (`PermissionsHook`) and the TUI `App`, so an
    /// "Always allow/deny" added in the Ask modal gates the next tool call
    /// without a restart (#55). Always present, even when the gate is
    /// disabled — the TUI still needs a store for the modal/overlay.
    pub runtime_rules: Arc<caliban_agent_core::RuntimeRuleStore>,
    /// The config-expressible permission policy (rules + mode + audit) that
    /// a background sub-agent inherits when `inherit_hooks=true` (#84).
    /// `None` when permissions are disabled (`--no-permissions`).
    pub inheritable_config: Option<crate::hook_inherit::InheritableHookConfig>,
}

/// Build the permissions chain (rules → `ModeFilter` → `PermissionsHook`).
///
/// Layers CLI flags (`--allow` / `--deny` / `--ask`) on top of the rules
/// projected from the layered `Settings` snapshot (`settings.json` plus
/// the legacy `permissions.toml` compat fallback already folded in by
/// `caliban-settings`), then appends the built-in `default_rules` tail
/// and folds per-server `[server.X.permissions]` blocks.
///
/// Returns `PermissionsSetup::default`-equivalent (all-`None`) when
/// `--no-permissions` is set.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_permissions(
    args: &Args,
    settings_snapshot: &caliban_settings::Settings,
    mcp_server_cfg: &std::collections::BTreeMap<String, caliban_mcp_client::ServerConfig>,
    provider: &Arc<dyn Provider + Send + Sync>,
    model: &str,
    permission_mode: &caliban_agent_core::SharedPermissionMode,
    tui_mode_active: bool,
) -> PermissionsSetup {
    use caliban_agent_core::{
        Action, AutoModeClassifier, AutoModeConfig, DEFAULTS_TOKEN, ModeFilter,
        NonInteractiveAskHandler, NoopHooks, PermissionsHook, Rule, default_rules,
    };

    if args.no_permissions {
        return PermissionsSetup {
            permissions_hook: None,
            tui_ask_rx: None,
            auto_mode_classifier: None,
            runtime_rules: Arc::new(caliban_agent_core::RuntimeRuleStore::new()),
            inheritable_config: None,
        };
    }
    let mut cli_rules: Vec<Rule> = Vec::new();
    for p in &args.allow {
        cli_rules.push(Rule {
            tool: p.clone(),
            action: Action::Allow,
            comment: None,
            reason: None,
            expires_at: None,
        });
    }
    for p in &args.deny {
        cli_rules.push(Rule {
            tool: p.clone(),
            action: Action::Deny,
            comment: None,
            reason: None,
            expires_at: None,
        });
    }
    for p in &args.ask {
        cli_rules.push(Rule {
            tool: p.clone(),
            action: Action::Ask,
            comment: None,
            reason: None,
            expires_at: None,
        });
    }
    // Layer Settings permission rules (which already incorporate the
    // legacy permissions.toml via the caliban-settings compat shim) at
    // lower priority than CLI flags. The built-in default-rules tail
    // closes the chain (catch-all `*` Ask).
    let mut global_rules = cli_rules;
    for r in settings_snapshot.permission_rules() {
        global_rules.push(r);
    }
    global_rules.extend(default_rules());
    // Phase B: fold per-server `[server.X.permissions]` blocks into the
    // global rule list at the documented priority slot
    // (global deny → server deny/ask/allow → global ask/allow → default).
    let rules = caliban_mcp_client::merge_with_global(global_rules, mcp_server_cfg);
    // Clone the resolved rule list before it is consumed by PermissionsHook::new
    // so background sub-agents can inherit it via InheritableHookConfig (#84).
    let inheritable_rules = rules.clone();
    // In interactive (TUI) mode, route Ask through the modal bridge. In
    // headless/single-prompt mode, fall back to the non-interactive handler.
    let (ask, ask_rx): (Arc<dyn caliban_agent_core::AskHandler>, _) = if tui_mode_active {
        let (handler, rx) = tui::TuiAskHandler::pair();
        (Arc::new(handler), Some(rx))
    } else {
        (
            Arc::new(NonInteractiveAskHandler {
                auto_allow: args.auto_allow,
            }),
            None,
        )
    };
    // Shared runtime-rule store: the gate consults it before the static
    // rule set, and the TUI appends to this same `Arc` from the Ask modal's
    // "Always allow/deny" branches (#55).
    let runtime_rules = Arc::new(caliban_agent_core::RuntimeRuleStore::new());
    let inner: Arc<dyn caliban_agent_core::Hooks> = Arc::new(
        PermissionsHook::new(rules, ask, Arc::new(NoopHooks))
            .with_runtime_rules(Arc::clone(&runtime_rules)),
    );

    // Build the auto-mode classifier. The provider is the same one wired
    // for the agent; when it's a router, FastClassifier requests route
    // to whichever model the operator configured for that purpose.
    let auto_cfg = AutoModeConfig {
        environment: vec![DEFAULTS_TOKEN.into()],
        allow: vec![DEFAULTS_TOKEN.into()],
        soft_deny: vec![DEFAULTS_TOKEN.into()],
        hard_deny: vec![DEFAULTS_TOKEN.into()],
        disabled: args.disable_auto_mode,
    };
    let classifier = Arc::new(AutoModeClassifier::new(
        Arc::clone(provider),
        model,
        auto_cfg,
    ));

    let filter: Arc<dyn caliban_agent_core::Hooks + Send + Sync> = Arc::new(ModeFilter::new(
        permission_mode.clone(),
        inner,
        Some(Arc::clone(&classifier)),
        args.allow_dangerously_skip_permissions,
    ));

    let session_id = args
        .session
        .clone()
        .or_else(|| args.resume.clone())
        .unwrap_or_else(|| "ephemeral".into());
    let audit_enabled = settings_snapshot.permissions.audit_log.unwrap_or(true);
    let hooks_chain = wrap_with_audit(filter, audit_enabled, session_id);

    PermissionsSetup {
        permissions_hook: Some(hooks_chain),
        tui_ask_rx: ask_rx,
        auto_mode_classifier: Some(classifier),
        runtime_rules,
        inheritable_config: Some(crate::hook_inherit::InheritableHookConfig {
            rules: inheritable_rules,
            mode: permission_mode.load(),
            audit: audit_enabled,
            runtime_rules: Vec::new(),
        }),
    }
}

/// Optionally wrap `inner` with a [`caliban_agent_core::decision_log::DecisionRecorder`]
/// when `audit_enabled` is true and the log path is resolvable.
pub(crate) fn wrap_with_audit(
    inner: Arc<dyn caliban_agent_core::Hooks + Send + Sync>,
    audit_enabled: bool,
    session_id: String,
) -> Arc<dyn caliban_agent_core::Hooks + Send + Sync> {
    if !audit_enabled {
        return inner;
    }
    let Some(path) = caliban_agent_core::decision_log::decision_log_path() else {
        return inner;
    };
    match caliban_agent_core::decision_log::DecisionLogWriter::open(path, session_id) {
        Ok(w) => Arc::new(caliban_agent_core::decision_log::DecisionRecorder {
            writer: Arc::new(w),
            inner,
            enabled: true,
        }),
        Err(e) => {
            tracing::warn!(error = %e, "audit log unavailable; proceeding without it");
            inner
        }
    }
}

/// Returns `Err` with a human-readable explanation when `enforce = true` is
/// set and the caller has flags that would weaken or skip permissions.
pub(crate) fn check_enforce_gate(
    args: &Args,
    settings: &caliban_settings::Settings,
) -> std::result::Result<(), String> {
    if settings.permissions.enforce != Some(true) {
        return Ok(());
    }
    if args.no_permissions {
        return Err("permissions.enforce = true is set; --no-permissions is refused".into());
    }
    if args.auto_allow {
        return Err("permissions.enforce = true is set; --auto-allow is refused".into());
    }
    // Reject every startup mode that materially weakens permissions:
    //  - bypassPermissions skips the rules entirely (latch is overridden here),
    //  - dontAsk rewrites every Ask -> Allow,
    //  - acceptEdits auto-allows all file edits.
    // A neutral mode (default/plan/auto) is left to run. See #178.
    if let Some(mode) = args.permission_mode.as_deref()
        && matches!(mode, "bypassPermissions" | "dontAsk" | "acceptEdits")
    {
        return Err(format!(
            "permissions.enforce = true is set; --permission-mode {mode} is refused \
             (it would weaken the enforced policy)"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod enforce_tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn enforce_true_blocks_no_permissions() {
        let mut settings = caliban_settings::Settings::default();
        settings.permissions.enforce = Some(true);
        let args = Args::try_parse_from(["caliban", "--no-permissions"]).unwrap();
        let result = check_enforce_gate(&args, &settings);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("enforce") && msg.contains("--no-permissions"),
            "expected enforce-blocks message, got: {msg}"
        );
    }

    #[test]
    fn enforce_false_or_unset_allows_no_permissions() {
        let settings = caliban_settings::Settings::default();
        let args = Args::try_parse_from(["caliban", "--no-permissions"]).unwrap();
        assert!(check_enforce_gate(&args, &settings).is_ok());
    }

    #[test]
    fn enforce_true_blocks_weakening_permission_modes() {
        // #178: dontAsk rewrites every Ask->Allow and acceptEdits auto-allows
        // file edits; both materially weaken permissions, so an enterprise
        // enforce=true policy must refuse them like bypassPermissions.
        for mode in ["dontAsk", "acceptEdits"] {
            let mut settings = caliban_settings::Settings::default();
            settings.permissions.enforce = Some(true);
            let args = Args::try_parse_from(["caliban", "--permission-mode", mode]).unwrap();
            let result = check_enforce_gate(&args, &settings);
            assert!(
                result.is_err(),
                "enforce=true must refuse --permission-mode {mode}"
            );
            assert!(
                result.unwrap_err().contains(mode),
                "refusal message should name the {mode} mode"
            );
        }
    }

    #[test]
    fn enforce_true_allows_neutral_permission_modes() {
        // default/plan do not weaken permissions and must still start.
        for mode in ["default", "plan"] {
            let mut settings = caliban_settings::Settings::default();
            settings.permissions.enforce = Some(true);
            let args = Args::try_parse_from(["caliban", "--permission-mode", mode]).unwrap();
            assert!(
                check_enforce_gate(&args, &settings).is_ok(),
                "enforce=true should still allow --permission-mode {mode}"
            );
        }
    }
}

/// Fire the `session_start` hook with the standard session context. Errors are
/// logged-and-swallowed (best-effort). Returns any `additional_context` blocks
/// the `SessionStart` hooks supplied, for splicing into the system prompt before
/// turn 1 (#106).
pub(crate) async fn fire_session_start(
    args: &Args,
    agent: &Arc<Agent>,
    model: &str,
) -> Vec<String> {
    let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let session_id = args.session.clone().unwrap_or_else(|| "ephemeral".into());
    let session_ctx = caliban_agent_core::SessionCtx {
        session_id: &session_id,
        cwd: &cwd_now,
        provider: provider_name(resolved_provider(args)),
        model,
    };
    match agent.hooks().session_start(&session_ctx).await {
        Ok(outcome) => outcome.additional_context,
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_start hook error (non-fatal)");
            Vec::new()
        }
    }
}

/// Fire the `session_end` hook with the standard session context.
/// Errors are logged-and-swallowed (best-effort).
pub(crate) async fn fire_session_end(
    args: &Args,
    agent: &Arc<Agent>,
    model: &str,
    total_usage: &Usage,
) {
    let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let session_id = args.session.clone().unwrap_or_else(|| "ephemeral".into());
    let session_ctx = caliban_agent_core::SessionCtx {
        session_id: &session_id,
        cwd: &cwd_now,
        provider: provider_name(resolved_provider(args)),
        model,
    };
    let outcome = caliban_agent_core::SessionOutcome {
        turn_count: 0, // not tracked at this layer; populated from final_messages by callers.
        input_tokens: total_usage.input_tokens,
        output_tokens: total_usage.output_tokens,
    };
    if let Err(e) = agent.hooks().session_end(&session_ctx, &outcome).await {
        tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_end hook error (non-fatal)");
    }
}

/// Build the agent: wire the provider + registry, install the output-
/// style post-processor when the `Learning` style is active, compose the
/// hook chain (`HeadlessHookSink` + `PermissionsHook`), and apply the
/// `--parallel-tool-limit` / `--temperature` knobs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_agent(
    args: &Args,
    provider: Arc<dyn Provider + Send + Sync>,
    registry: ToolRegistry,
    model: &str,
    plan_mode: &caliban_agent_core::SharedPlanMode,
    permissions_hook: Option<Arc<dyn caliban_agent_core::Hooks + Send + Sync>>,
    hook_event_buffer: Option<&headless::HookEventBuffer>,
    settings_snapshot: &caliban_settings::Settings,
    hooks_cfg: &caliban_agent_core::HooksConfig,
    mcp_active: Arc<arc_swap::ArcSwap<caliban_agent_core::mcp_activation::McpActivationSet>>,
    mcp_eager_servers: Arc<std::collections::HashSet<String>>,
) -> Result<Arc<Agent>> {
    // ADR-0046: resolve lazy_mcp / max_active_schemas from settings.
    // Builder defaults match the spec (off / 24) when absent.
    let (lazy_mcp, max_active_schemas) = match settings_snapshot.tools.as_ref() {
        Some(t) => (
            t.lazy_mcp.unwrap_or(false),
            t.max_active_schemas.unwrap_or(24),
        ),
        None => (false, 24),
    };
    // CLI > settings > built-in default for max_tokens_recovery.
    let max_tokens_recovery = args
        .max_tokens_recovery
        .or(settings_snapshot.max_tokens_recovery)
        .unwrap_or_else(|| caliban_agent_core::AgentConfig::default().max_tokens_recovery);
    let mut cfg = caliban_agent_core::AgentConfig {
        model: model.to_string(),
        max_tokens: args.max_tokens,
        max_turns: args.max_turns,
        max_tokens_recovery,
        lazy_mcp,
        max_active_schemas,
        ..caliban_agent_core::AgentConfig::default()
    };
    // Plan B context-management knobs from Settings — auto_compact_threshold,
    // micro_compact_enabled, tool_result_cap_chars, min_cache_block_tokens.
    // Without this call the four fields parse off disk but never reach the
    // agent (PR #60 introduced both the Settings fields and this helper but
    // the wiring step was missed). Sub-agent inheritance for these knobs is
    // a separate follow-up — install_sub_agent does not yet thread the same
    // Settings snapshot into the factory closure.
    settings_snapshot.apply_context_management(&mut cfg);
    let mut builder = Agent::builder()
        .provider(provider)
        .tools(registry)
        .config(cfg)
        .prompt_cache(!args.no_prompt_cache)
        .parallel_tools(!args.no_parallel_tools)
        .plan_mode(Arc::clone(plan_mode))
        .mcp_active(mcp_active)
        .mcp_eager_servers(mcp_eager_servers);
    // Install the output-style post-processor. Today only the `Learning`
    // style mutates assistant text; everything else uses the identity
    // post-processor (which the agent core already defaults to).
    {
        let workspace_root_for_style = args.workspace.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        let style_reg =
            caliban_output_styles::OutputStylesRegistry::load(&workspace_root_for_style);
        let requested = caliban_output_styles::requested_from_env();
        // v2: enabled_plugins is empty until ADR 0030 plugin system ships.
        let active = style_reg.select(&requested, &[]);
        if let Some(s) = active.as_ref()
            && s.name == "learning"
        {
            let pp: Arc<dyn caliban_agent_core::AssistantPostProcessor> =
                Arc::new(caliban_output_styles::LearningPostProcessor::new());
            builder = builder.post_processor(pp);
        }
    }
    // Compose hooks. When `--include-hook-events` is set, attach a
    // `HeadlessHookSink` at the outermost position so every event becomes
    // an observable frame (ADR 0025). Headless-only — TUI mode ignores it.
    {
        let mut layers: Vec<Arc<dyn caliban_agent_core::Hooks>> = Vec::new();
        if let Some(buf) = hook_event_buffer {
            layers.push(Arc::new(headless::HeadlessHookSink::new(Arc::clone(buf))));
        }
        // Config-defined `[[hooks.*]]` handlers (#121), inserted after the
        // observability sink and before the permission gate so a config
        // `PreToolUse` deny short-circuits the permission check.
        for h in caliban_agent_core::build_config_hooks(hooks_cfg, &web_fetch_client()) {
            layers.push(h);
        }
        if let Some(p) = permissions_hook {
            // PermissionsHook is `Send + Sync` but CompositeHooks accepts
            // `Arc<dyn Hooks>` (the trait bound is `Send + Sync` on the
            // supertrait), so coerce.
            layers.push(p as Arc<dyn caliban_agent_core::Hooks>);
        }
        if !layers.is_empty() {
            let composite: Arc<dyn caliban_agent_core::Hooks + Send + Sync> =
                Arc::new(caliban_agent_core::CompositeHooks::new(layers));
            builder = builder.hooks(composite);
        }
    }
    if let Some(limit) = args.parallel_tool_limit {
        builder = builder.parallel_tool_limit(limit);
    }
    if let Some(t) = args.temperature {
        builder = builder.temperature(t);
    }
    Ok(Arc::new(builder.build()?))
}

/// Resolve the session store and load (or create) the persisted session.
/// Seeds the shared todos handle + plan-mode flag from the persisted
/// snapshot if any.
pub(crate) fn resolve_session(
    args: &Args,
    model: &str,
    todos: &caliban_agent_core::SharedTodos,
    plan_mode: &caliban_agent_core::SharedPlanMode,
) -> Result<(Option<SessionStore>, Option<PersistedSession>)> {
    // Build the session store whenever any flag actually needs one:
    // `--session <NAME>` (legacy), `--continue`, or `--resume <NAME>`.
    // Without this, `--sessions-dir <X> --continue` (no `--session`) would
    // silently fall back to scanning `~/.caliban/sessions` and find nothing,
    // then no-op into a fresh ephemeral run — exactly Finding 11 of the
    // 2026-05-27 LM Studio probe.
    let needs_store = args.session.is_some() || args.continue_latest || args.resume.is_some();
    let store = if needs_store {
        Some(SessionStore::new(match &args.sessions_dir {
            Some(d) => d.clone(),
            None => SessionStore::default_root()?,
        }))
    } else {
        None
    };
    let session = if let (Some(store), Some(name)) = (&store, &args.session) {
        Some(match store.load(name)? {
            Some(existing) => existing,
            None => PersistedSession::new(
                name.clone(),
                provider_name(resolved_provider(args)),
                model.to_string(),
            ),
        })
    } else {
        None
    };
    if let Some(sess) = session.as_ref() {
        todos
            .lock()
            .expect("todos lock poisoned")
            .clone_from(&sess.todos);
        plan_mode.store(sess.plan_mode, std::sync::atomic::Ordering::Relaxed);
    }
    Ok((store, session))
}

/// Resolve the effective system prompt for this run. Handles the
/// `--system` / `--system-file` / `--no-system` overrides, then (when
/// the default prompt is in effect) layers the active output-style
/// block and the memory-tier prefix on top.
pub(crate) async fn resolve_system_prompt(
    args: &Args,
    agent: &Arc<Agent>,
    cwd_for_prompt: &std::path::Path,
    settings_snapshot: &caliban_settings::Settings,
    session_context: &[String],
) -> Result<Option<String>> {
    let tool_names: Vec<&str> = agent.tools().names().collect();
    let default_prompt_in_effect =
        args.system.is_none() && args.system_file.is_none() && !args.no_system;
    let system_prompt = system_prompt::resolve(
        args.system.as_deref(),
        args.system_file.as_deref(),
        args.no_system,
        cwd_for_prompt,
        &tool_names,
        args.no_tools,
    )?;

    // Load memory tiers and splice into the default system prompt, then
    // wrap with the active output-style block (after memory, before the
    // base body). The operator's --system / --system-file / --no-system
    // always wins — those paths intentionally skip both memory and output
    // styles.
    let Some(body) = system_prompt else {
        return Ok(None);
    };

    // Proactive skill-invocation nudge (#56): list loaded skill names so the
    // model invokes a matching skill instead of improvising. Gated by
    // `tools.skill_guidance`; empty when disabled or no skills are loaded. The
    // block is appended at the tail of whatever prompt is in effect (default or
    // custom), so it survives output-style/memory layering.
    let skill_names = proactive_skill_names(agent, settings_snapshot);

    if !default_prompt_in_effect {
        let with_skills = system_prompt::append_skills_block(&body, &skill_names);
        return Ok(Some(system_prompt::append_session_context_block(
            &with_skills,
            session_context,
        )));
    }

    let workspace_root = args
        .workspace
        .clone()
        .unwrap_or_else(|| cwd_for_prompt.to_path_buf());

    // Resolve the active output style. Selection precedence:
    //   1. `force_for_plugin` on a plugin-supplied style (v2 — inert
    //      until ADR 0030 plugin system lands).
    //   2. `CALIBAN_OUTPUT_STYLE` env var (settings.json key with
    //      ADR 0026).
    //   3. Built-in `default` (no-op).
    let style_registry = caliban_output_styles::OutputStylesRegistry::load(&workspace_root);
    let requested = caliban_output_styles::requested_from_env();
    // v2: enabled_plugins is empty until ADR 0030 ships the plugin system.
    let enabled_plugins: Vec<String> = Vec::new();
    let active_style = style_registry.select(&requested, &enabled_plugins);
    let style_prefix = caliban_output_styles::OutputStylePrefix::new(active_style.clone());

    // When the active style requests `keep_coding_instructions: false`,
    // replace the default coding-assistant body with the style body so
    // the prompt does not double up on guidance. The style body is
    // already wrapped in `<output-style>` tags by `splice_into`, so we
    // just feed an empty `base` to the splice.
    let base_body = if style_prefix.drops_coding_instructions() {
        String::new()
    } else {
        body
    };

    // Layering: memory tiers first (highest cache-key precedence), then
    // the output-style block, then the base body. We construct from the
    // inside out — wrap the base body with the style prefix, then wrap
    // that with the memory prefix.
    let with_style = style_prefix.splice_into(&base_body);
    // --bare (ADR 0025) skips auto-memory load entirely.
    let final_prompt = if args.bare {
        with_style
    } else {
        let cfg = apply_memory_settings(
            caliban_memory::MemoryConfig::from_env(&workspace_root),
            settings_snapshot,
        );
        match caliban_memory::load(&cfg).await {
            Ok(prefix) => prefix.splice_into(&with_style),
            Err(e) => {
                tracing::warn!(target: caliban_common::tracing_targets::TARGET_MEMORY, error = %e, "memory load failed; using default prompt without memory");
                with_style
            }
        }
    };
    let with_skills = system_prompt::append_skills_block(&final_prompt, &skill_names);
    Ok(Some(system_prompt::append_session_context_block(
        &with_skills,
        session_context,
    )))
}

/// Skill names to surface in the system prompt's proactive-invocation block,
/// honoring the `tools.skill_guidance` opt-out (#56). Returns an empty list when
/// guidance is disabled or no `Skill` tool is registered (e.g. `--no-skills`,
/// `--bare`, `--no-tools`), in which case no block is injected.
fn proactive_skill_names<'a>(
    agent: &'a Agent,
    settings: &caliban_settings::Settings,
) -> Vec<&'a str> {
    let disabled = settings.tools.as_ref().and_then(|t| t.skill_guidance) == Some(false);
    if disabled {
        return Vec::new();
    }
    agent
        .tools()
        .get("Skill")
        .and_then(|t| t.as_any())
        .and_then(|a| a.downcast_ref::<SkillTool>())
        .map(SkillTool::skill_names_sorted)
        .unwrap_or_default()
}

/// Overlay `settings.memory.cap_tokens_*` (when present) onto a `MemoryConfig`
/// built from env defaults. Settings values take precedence over env vars when
/// both are set; missing settings keys leave the env-derived value in place.
///
/// Honored keys (all integer, non-negative):
/// - `memory.cap_tokens_combined` → `max_tokens`
/// - `memory.cap_tokens_auto` → per-scope auto-tier cap
/// - `memory.cap_tokens_claude_md` → per-scope CLAUDE.md-tier cap (global + project)
fn apply_memory_settings(
    mut cfg: caliban_memory::MemoryConfig,
    settings_snapshot: &caliban_settings::Settings,
) -> caliban_memory::MemoryConfig {
    let Some(memory) = settings_snapshot.memory.as_ref() else {
        return cfg;
    };
    let read_usize = |key: &str| {
        memory
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    if let Some(n) = read_usize("cap_tokens_combined") {
        cfg.max_tokens = n;
    }
    if let Some(n) = read_usize("cap_tokens_auto") {
        cfg = cfg.with_cap_tokens_auto(n);
    }
    if let Some(n) = read_usize("cap_tokens_claude_md") {
        cfg = cfg.with_cap_tokens_claude_md(n);
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::{debug_enabled, missing_key_err, resolve_debug_log_path};
    use crate::args::Args;
    use clap::Parser as _;

    /// Parse an `Args` from the given extra flags (always with `caliban` as
    /// argv[0]). Mirrors the helper in `args.rs`'s own test module.
    fn parse_args(extra: &[&str]) -> Args {
        let mut argv: Vec<&str> = vec!["caliban"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).expect("clap parse")
    }

    #[tokio::test]
    async fn session_context_is_spliced_into_prompt() {
        use super::resolve_system_prompt;
        use caliban_agent_core::{Agent, ToolRegistry};
        use caliban_provider::{MockProvider, Provider};
        use std::sync::Arc;

        // Minimal offline agent (no network) with an empty tool registry, so
        // the default coding-assistant prompt is in effect and no skills block
        // is emitted.
        let provider: Arc<dyn Provider + Send + Sync> = Arc::new(MockProvider::new());
        let agent = Arc::new(
            Agent::builder()
                .provider(provider)
                .tools(ToolRegistry::new())
                .model("mock")
                .max_tokens(64)
                .max_turns(1)
                .build()
                .expect("agent builder"),
        );
        // `--bare` skips memory load so the test is hermetic.
        let args = parse_args(&["--bare"]);
        let settings = caliban_settings::Settings::default();
        let cwd = std::env::current_dir().unwrap();

        let with_ctx = resolve_system_prompt(
            &args,
            &agent,
            &cwd,
            &settings,
            &["INJECTED-MARKER".to_string()],
        )
        .await
        .unwrap()
        .expect("default prompt in effect");
        assert!(
            with_ctx.contains("<session-context>"),
            "session-context block should be present when context is supplied"
        );
        assert!(with_ctx.contains("INJECTED-MARKER"));

        let without_ctx = resolve_system_prompt(&args, &agent, &cwd, &settings, &[])
            .await
            .unwrap()
            .expect("default prompt in effect");
        assert!(
            !without_ctx.contains("<session-context>"),
            "no session-context block when no context is supplied"
        );
    }

    #[test]
    fn debug_disabled_without_any_flag() {
        // Guard on the ambient env var so a dev with CALIBAN_DEBUG exported
        // doesn't make this flake (issue #41 territory).
        if std::env::var_os("CALIBAN_DEBUG").is_none()
            && std::env::var_os("CALIBAN_DEBUG_FILE").is_none()
        {
            assert!(!debug_enabled(&parse_args(&[])));
            assert!(resolve_debug_log_path(&parse_args(&[])).is_none());
        }
    }

    #[test]
    fn debug_file_override_wins() {
        let args = parse_args(&["--debug-file", "/tmp/caliban-test.log"]);
        assert_eq!(
            resolve_debug_log_path(&args),
            Some(std::path::PathBuf::from("/tmp/caliban-test.log")),
        );
    }

    #[test]
    fn debug_file_implies_debug_on() {
        // No `--debug`, just `--debug-file` — logging must still turn on.
        let args = parse_args(&["--debug-file", "/tmp/caliban-test.log"]);
        assert!(debug_enabled(&args));
    }

    #[test]
    fn debug_flag_keeps_default_path() {
        let args = parse_args(&["--debug"]);
        assert!(debug_enabled(&args));
        // dirs::cache_dir() is Some on supported platforms; when it is, the
        // default destination is unchanged.
        if let Some(path) = resolve_debug_log_path(&args) {
            assert!(
                path.ends_with("caliban/debug.log"),
                "default path should be <cache>/caliban/debug.log; got {}",
                path.display()
            );
        }
    }

    #[test]
    fn missing_key_err_names_the_env_var() {
        let msg = missing_key_err("ANTHROPIC_API_KEY").to_string();
        assert!(msg.contains("ANTHROPIC_API_KEY"), "got: {msg}");
        assert!(
            msg.contains("apiKeyHelper"),
            "should hint at the helper path: {msg}"
        );
    }
}
