//! Router wiring for the caliban binary.
//!
//! Bridges between `caliban-model-router`'s config view and concrete adapter
//! constructors. Closes ADR 0038's "binary wiring" deferral: when
//! `caliban.toml` is present (auto-discovered, env-pinned, or `--config`-
//! flagged), the binary builds a [`ModelRouter`] from it; otherwise it
//! falls back to the single-provider construction path the user already had.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use caliban_model_router::{
    DerivedNeeds, DiscoveredConfig, EffortLevel, ModelRouter, ProviderBlock, RouterConfig,
    discover_caliban_toml, render_diagnostics,
};
use caliban_provider::{Provider, RequestPurpose};

/// Result of attempting to wire the router from `caliban.toml`.
#[derive(Debug)]
pub(crate) struct RouterWiring {
    /// The constructed router.
    pub router: Arc<ModelRouter>,
    /// Path the config was loaded from (for `[caliban] init` log lines).
    pub config_path: std::path::PathBuf,
}

/// Try to discover + build a router from `caliban.toml`. Returns `Ok(None)`
/// if no config is found anywhere; the caller falls back to the single-
/// provider path. `pool` supplies API keys via `api_key_helper` when
/// configured; an empty pool falls through to the env-var path.
pub(crate) fn try_load(
    explicit: Option<&Path>,
    start_dir: &Path,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<Option<RouterWiring>> {
    // The discovery error already names the path and the parse failure;
    // no extra context is needed (M5 doc-chain dedup).
    let Some(discovered) = discover_caliban_toml(explicit, start_dir).map_err(|e| anyhow!(e))?
    else {
        return Ok(None);
    };
    let DiscoveredConfig { path, config } = discovered;
    let Some(router_cfg) = config.router.clone() else {
        // caliban.toml exists but doesn't define [router].
        return Ok(None);
    };
    let providers = build_provider_handles(&router_cfg, &config.providers, pool)?;
    let router = ModelRouter::from_config(router_cfg, providers)
        .with_context(|| format!("building ModelRouter from {}", path.display()))?;
    Ok(Some(RouterWiring {
        router: Arc::new(router),
        config_path: path,
    }))
}

/// Build a provider handle for every name referenced by the routes.
pub(crate) fn build_provider_handles(
    router_cfg: &RouterConfig,
    provider_blocks: &HashMap<String, ProviderBlock>,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<HashMap<String, Arc<dyn Provider + Send + Sync>>> {
    let mut names: Vec<&str> = router_cfg
        .routes
        .iter()
        .map(|r| r.provider.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();

    let mut out: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
    for name in names {
        let block = provider_blocks.get(name).cloned().unwrap_or_default();
        let handle = build_one(name, &block, pool)
            .with_context(|| format!("constructing provider '{name}'"))?;
        out.insert(name.to_string(), handle);
    }
    Ok(out)
}

/// Resolve the API key for `(provider_id, api_key_env)`. Helper wins
/// when configured; env var is the fallback.
fn resolve_key(
    provider_id: &str,
    api_key_env: &str,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<secrecy::SecretString> {
    if pool.has_spec_for(provider_id) {
        let outcome = pool
            .key_for(provider_id)
            .map_err(|e| anyhow!("api_key_helper for {provider_id}: {e}"))?;
        Ok(secrecy::SecretString::from(outcome.key))
    } else {
        let key = std::env::var(api_key_env)
            .with_context(|| format!("env var {api_key_env} is unset"))?;
        Ok(secrecy::SecretString::from(key))
    }
}

fn build_one(
    name: &str,
    block: &ProviderBlock,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<Arc<dyn Provider + Send + Sync>> {
    match name {
        "anthropic" => {
            use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
            let api_key_env = block.api_key_env.as_deref().unwrap_or("ANTHROPIC_API_KEY");
            let base_url = block.base_url.clone();
            let make_cfg = move |key: secrecy::SecretString| -> Result<DirectConfig> {
                let mut cfg = DirectConfig::new(key);
                if let Some(url) = base_url.as_ref() {
                    cfg.base_url = url::Url::parse(url)?;
                }
                Ok(cfg)
            };
            let key = resolve_key("anthropic", api_key_env, pool)?;
            let inner = AnthropicProvider::direct(make_cfg(key)?)?;
            Ok(wrap_with_refresh_if_helper(
                inner,
                pool,
                "anthropic",
                "anthropic",
                move |k| {
                    let cfg =
                        make_cfg(k).map_err(|e| caliban_provider::Error::Adapter(e.into()))?;
                    AnthropicProvider::direct(cfg).map_err(caliban_provider::Error::adapter)
                },
            ))
        }
        "openai" => {
            use caliban_provider_openai::{OpenAIProvider, config::DirectConfig};
            let api_key_env = block.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
            let base_url = block.base_url.clone();
            let make_cfg = move |key: secrecy::SecretString| -> Result<DirectConfig> {
                let mut cfg = DirectConfig::new(key);
                if let Some(url) = base_url.as_ref() {
                    cfg.base_url = url::Url::parse(url)?;
                }
                Ok(cfg)
            };
            let key = resolve_key("openai", api_key_env, pool)?;
            let inner = OpenAIProvider::direct(make_cfg(key)?)?;
            Ok(wrap_with_refresh_if_helper(
                inner,
                pool,
                "openai",
                "openai",
                move |k| {
                    let cfg =
                        make_cfg(k).map_err(|e| caliban_provider::Error::Adapter(e.into()))?;
                    OpenAIProvider::direct(cfg).map_err(caliban_provider::Error::adapter)
                },
            ))
        }
        "ollama" => {
            use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
            let mut cfg = DirectConfig::new();
            if let Some(url) = block.base_url.as_ref() {
                cfg.base_url = url::Url::parse(url)?;
            } else if let Ok(c) = DirectConfig::from_env() {
                cfg = c;
            }
            Ok(Arc::new(OllamaProvider::direct(cfg)?))
        }
        "google" => {
            use caliban_provider_google::{GoogleProvider, config::AIStudioConfig};
            let api_key_env = block.api_key_env.as_deref().unwrap_or("GEMINI_API_KEY");
            // base_url override is provider-specific; ignored for AI Studio's
            // fixed endpoint in v2 (operator can pin via env vars).
            let _ = block.base_url;
            let key = resolve_key("google", api_key_env, pool)?;
            let inner = GoogleProvider::ai_studio(AIStudioConfig::new(key))?;
            Ok(wrap_with_refresh_if_helper(
                inner,
                pool,
                "google",
                "google",
                move |k| {
                    GoogleProvider::ai_studio(AIStudioConfig::new(k))
                        .map_err(caliban_provider::Error::adapter)
                },
            ))
        }
        other => Err(anyhow!(
            "unknown provider '{other}' — supported: anthropic, openai, ollama, google"
        )),
    }
}

/// Wrap `inner` in a `RefreshingProvider` iff the pool has a spec for
/// `provider_id`. Without a spec, no refresh path is needed and the
/// inner provider is returned as-is.
fn wrap_with_refresh_if_helper<P>(
    inner: P,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
    provider_id: &str,
    static_name: &'static str,
    rebuild: impl Fn(secrecy::SecretString) -> std::result::Result<P, caliban_provider::Error>
    + Send
    + Sync
    + 'static,
) -> Arc<dyn Provider + Send + Sync>
where
    P: Provider + 'static,
{
    if pool.has_spec_for(provider_id) {
        Arc::new(crate::refreshing_provider::RefreshingProvider::new(
            inner,
            pool.clone(),
            provider_id.to_string(),
            static_name,
            rebuild,
        ))
    } else {
        Arc::new(inner)
    }
}

/// CLI shape for `caliban router debug`.
#[derive(Debug, clap::Args, Clone)]
pub(crate) struct RouterDebugArgs {
    /// Purpose to resolve for.
    #[arg(long, default_value = "main_loop")]
    pub purpose: String,
    /// Pretend the request has a vision/image block.
    #[arg(long)]
    pub has_vision: bool,
    /// Pretend the request has tools attached.
    #[arg(long)]
    pub has_tools: bool,
    /// Pretend the request has a thinking budget.
    #[arg(long)]
    pub has_thinking: bool,
    /// Effort knob to surface.
    #[arg(long)]
    pub effort: Option<String>,
}

/// Parse the `--purpose` flag into a [`RequestPurpose`].
pub(crate) fn parse_purpose(s: &str) -> Result<RequestPurpose> {
    Ok(match s {
        "main_loop" | "MainLoop" => RequestPurpose::MainLoop,
        "summarization" | "Summarization" => RequestPurpose::Summarization,
        "fast_classifier" | "FastClassifier" => RequestPurpose::FastClassifier,
        "sub_agent" | "SubAgent" => RequestPurpose::SubAgent,
        "embedding" | "Embedding" => RequestPurpose::Embedding,
        "other" | "Other" => RequestPurpose::Other,
        _ => return Err(anyhow!("unknown purpose '{s}'")),
    })
}

/// Parse `--effort` into an `EffortLevel`.
pub(crate) fn parse_effort(s: &str) -> Result<EffortLevel> {
    Ok(match s {
        "low" => EffortLevel::Low,
        "medium" => EffortLevel::Medium,
        "high" => EffortLevel::High,
        _ => return Err(anyhow!("unknown effort '{s}' (low|medium|high)")),
    })
}

/// Render the "no caliban.toml present" diagnostic. Kept separate from
/// [`run_debug`] so its main body stays under clippy's 100-line cap.
fn render_no_config(args: &RouterDebugArgs) -> Result<String> {
    use std::fmt::Write as _;
    let purpose = parse_purpose(&args.purpose)?;
    let needs = DerivedNeeds {
        vision: args.has_vision,
        tool_use: args.has_tools,
        thinking: args.has_thinking,
    };
    let mut out = String::new();
    writeln!(out, "no caliban.toml found — router unconfigured")?;
    writeln!(out, "purpose: {purpose:?}")?;
    writeln!(
        out,
        "derived needs: vision={} tools={} thinking={}",
        needs.vision, needs.tool_use, needs.thinking,
    )?;
    writeln!(
        out,
        "fallback: single-provider via --provider/--model (default: {}/{})",
        crate::args::provider_name(crate::args::ProviderKind::Anthropic),
        crate::args::default_model_for(crate::args::ProviderKind::Anthropic),
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "drop a `caliban.toml` into the repo root with a [router] section to enable \
         fallback / hedging / capability filtering (ADR 0038).",
    )?;
    Ok(out)
}

/// Execute `caliban router debug` — print the resolved candidate list for a
/// synthetic request matching the CLI flags.
///
/// When no `caliban.toml` is present the command still succeeds — instead
/// of erroring it prints the single-provider fallback the binary would
/// use, so the debug subcommand stays useful for the common no-router
/// case. The resolved purpose, derived needs, and effort hint still
/// show even without a router config.
pub(crate) fn run_debug(
    args: &RouterDebugArgs,
    explicit_config: Option<&Path>,
    start_dir: &Path,
) -> Result<String> {
    use std::fmt::Write as _;
    // Router debug uses an empty helper pool — diagnostics shouldn't
    // spawn external scripts.
    let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
    let Some(wiring) = try_load(explicit_config, start_dir, &empty_pool)? else {
        return render_no_config(args);
    };

    let purpose = parse_purpose(&args.purpose)?;
    let needs = DerivedNeeds {
        vision: args.has_vision,
        tool_use: args.has_tools,
        thinking: args.has_thinking,
    };

    // Build a synthetic request honoring the CLI flags.
    let mut req = caliban_provider::CompletionRequest {
        model: String::new(),
        messages: vec![caliban_provider::Message::user_text("(debug)")],
        tools: vec![],
        tool_choice: caliban_provider::ToolChoice::default(),
        max_tokens: 64,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: caliban_provider::ThinkingSetting::Auto,
        effort: None,
        metadata: caliban_provider::RequestMetadata {
            user_id: None,
            purpose: Some(purpose),
        },
    };
    if args.has_vision {
        req.messages = vec![caliban_provider::Message {
            role: caliban_provider::Role::User,
            content: vec![caliban_provider::ContentBlock::Image(
                caliban_provider::ImageBlock {
                    source: caliban_provider::ImageSource::Url {
                        url: "https://example.invalid/placeholder.png".into(),
                    },
                    cache_control: None,
                    sha256: None,
                    dims: None,
                },
            )],
        }];
    }
    if args.has_tools {
        req.tools = vec![caliban_provider::Tool {
            name: "T".into(),
            description: "placeholder".into(),
            input_schema: serde_json::json!({"type":"object"}),
            cache_control: None,
        }];
    }
    if args.has_thinking {
        req.thinking = caliban_provider::ThinkingSetting::On(Some(4096));
    }

    let (_cands, diagnostics) = wiring
        .router
        .resolve_diagnostics(&req)
        .context("resolving candidates")?;
    let mut out = format!("config: {}\n", wiring.config_path.display());
    out.push_str(&render_diagnostics(purpose, needs, &diagnostics));

    // Effort table for the resolved candidates.
    if let Some(level_s) = args.effort.as_deref() {
        let level = parse_effort(level_s)?;
        out.push_str("\neffort table:\n");
        for r in wiring.router.routes() {
            let _ = writeln!(
                out,
                "  {}: effort_map.{} = {}",
                r.id,
                level.as_str(),
                r.effort_knob_for(level)
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const MINIMAL_OLLAMA: &str = r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "ollama"
model = "llama3.2:3b"
fallback = []

[provider.ollama]
base_url = "http://localhost:11434"
"#;

    #[test]
    fn no_config_default_model_is_derived_not_hardcoded() {
        // Regression #144: the fallback-default line must reflect the real
        // default model (args::default_model_for), not a hardcoded id that
        // drifts when the default bumps.
        use crate::args::{ProviderKind, default_model_for, provider_name};
        let args = RouterDebugArgs {
            purpose: "main_loop".into(),
            has_vision: false,
            has_tools: false,
            has_thinking: false,
            effort: None,
        };
        let out = render_no_config(&args).unwrap();
        let expected = format!(
            "{}/{}",
            provider_name(ProviderKind::Anthropic),
            default_model_for(ProviderKind::Anthropic),
        );
        assert!(out.contains(&expected), "expected `{expected}` in:\n{out}");
        assert!(
            !out.contains("claude-3-5-sonnet"),
            "stale hardcoded model id present:\n{out}"
        );
    }

    #[test]
    fn debug_prints_candidate_list() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("caliban.toml"), MINIMAL_OLLAMA).unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let args = RouterDebugArgs {
            purpose: "main_loop".into(),
            has_vision: false,
            has_tools: false,
            has_thinking: false,
            effort: Some("high".into()),
        };
        let out = run_debug(&args, Some(&tmp.path().join("caliban.toml")), tmp.path()).unwrap();
        assert!(out.contains("ollama:llama3.2:3b:main_loop"), "got:\n{out}");
        assert!(out.contains("effort_map.high"), "got:\n{out}");
    }

    #[test]
    fn unknown_provider_string_fails_at_startup_loudly() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("caliban.toml"),
            r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "made-up-provider"
model = "x"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        let err = try_load(
            Some(&tmp.path().join("caliban.toml")),
            tmp.path(),
            &empty_pool,
        )
        .unwrap_err();
        let s = format!("{err:?}");
        assert!(s.contains("unknown provider"), "{s}");
    }
}
