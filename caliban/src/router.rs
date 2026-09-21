//! Router wiring for the caliban binary.
//!
//! Bridges between `caliban-model-router`'s config view and concrete adapter
//! constructors. Router config resolves through the caliban **settings** layer
//! (`.caliban/settings.toml` `[router]`, with the standard precedence chain),
//! with an explicit `--config <PATH>` / `CALIBAN_ROUTER_CONFIG` file as the
//! highest-precedence override; when neither is present the binary falls back to
//! the single-provider construction path (ADR 0038 binary wiring, ADR 0060
//! settings-sourced routing). The walk-up + home-dir `caliban.toml` *discovery*
//! was removed in #699.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use caliban_model_router::{
    DerivedNeeds, EffortLevel, LoadedConfig, ModelRouter, ProviderBlock, RouterConfig,
    load_router_config_file, render_diagnostics, router_and_providers_from_value,
};
use caliban_provider::{Provider, RequestPurpose};

use crate::provider_wiring::{resolve_key, resolve_key_optional, wrap_with_refresh_if_helper};

/// Result of attempting to wire the router from settings or `caliban.toml`.
#[derive(Debug)]
pub(crate) struct RouterWiring {
    /// The constructed router.
    pub router: Arc<ModelRouter>,
    /// Path the config was loaded from (for `[caliban] init` log lines). Empty
    /// when the config came from the settings layer rather than a file.
    pub config_path: std::path::PathBuf,
    /// True when the router config came from the settings-layer `[router]`
    /// section rather than an on-disk `caliban.toml` (#540).
    pub from_settings: bool,
}

/// Wire the router from the settings layer, with an explicit `--config` file as
/// the highest-precedence override (#540, #699).
///
/// Precedence:
/// 1. **Explicit `--config <PATH>` / `CALIBAN_ROUTER_CONFIG`** — when set, that
///    file wins outright (its `[router]` + `[provider.X]` blocks are used).
/// 2. **Settings-layer `[router]`** — the opaque settings value is typed into a
///    [`RouterConfig`] plus its `[router.provider.X]` blocks via
///    [`router_and_providers_from_value`] (keeping `caliban-settings` free of a
///    dependency on the router schema).
/// 3. **`Ok(None)`** — neither is present; the caller falls back to the
///    single-provider construction path.
///
/// The walk-up + home-dir `caliban.toml` discovery that used to sit between
/// (1) and (3) was removed in #699.
pub(crate) fn wire_router(
    settings_router: Option<&serde_json::Value>,
    explicit: Option<&Path>,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<Option<RouterWiring>> {
    // 1. Explicit --config / CALIBAN_ROUTER_CONFIG file wins.
    if let Some(loaded) = load_router_config_file(explicit).map_err(|e| anyhow!(e))? {
        let LoadedConfig { path, config } = loaded;
        let Some(router_cfg) = config.router.clone() else {
            // The explicit file exists but defines no [router].
            return Ok(None);
        };
        let providers = build_provider_handles(&router_cfg, &config.providers, pool)?;
        let router = ModelRouter::from_config(router_cfg, providers)
            .with_context(|| format!("building ModelRouter from {}", path.display()))?;
        return Ok(Some(RouterWiring {
            router: Arc::new(router),
            config_path: path,
            from_settings: false,
        }));
    }

    // 2. Settings-layer [router] section (+ [router.provider.X]).
    if let Some(value) = settings_router.filter(|v| !v.is_null()) {
        let (router_cfg, provider_blocks) = router_and_providers_from_value(value)
            .context("parsing the [router] section from settings")?;
        let providers = build_provider_handles(&router_cfg, &provider_blocks, pool)?;
        let router = ModelRouter::from_config(router_cfg, providers)
            .context("building ModelRouter from the settings [router] section")?;
        return Ok(Some(RouterWiring {
            router: Arc::new(router),
            config_path: std::path::PathBuf::new(),
            from_settings: true,
        }));
    }

    // 3. No router config anywhere → single-provider fallback (caller).
    Ok(None)
}

/// Emit a one-time warning when a `caliban.toml` sits above `start_dir` but is
/// no longer auto-loaded (router discovery was removed in #699).
///
/// `explicit_used` is true when the operator passed `--config` /
/// `CALIBAN_ROUTER_CONFIG` — in that case the file is being used deliberately,
/// so no warning is emitted. Otherwise a discoverable `caliban.toml` is now
/// silently ignored, so we point the operator at the migration path rather than
/// letting their routing config vanish without a word.
pub(crate) fn warn_if_orphaned_caliban_toml(start_dir: &Path, explicit_used: bool) {
    if explicit_used {
        return;
    }
    if let Some(found) = caliban_common::paths::walk_up_for_file(start_dir, "caliban.toml") {
        tracing::warn!(
            target: caliban_common::tracing_targets::TARGET_ROUTER,
            path = %found.display(),
            "found a caliban.toml that is no longer auto-loaded — router discovery was \
             removed (#699). Migrate it into .caliban/settings.toml with \
             `caliban config import-router`, or point at it explicitly with --config.",
        );
    }
}

/// Reshape a legacy `caliban.toml` body into the settings-layer `router` value
/// (the shape stored in `Settings.router`), moving top-level `[provider.X]`
/// blocks under `router.provider.X` (#699).
///
/// The result is validated by loading it exactly as the runtime will
/// ([`router_and_providers_from_value`]), so a malformed source fails loudly
/// instead of migrating garbage into `settings.toml`.
///
/// # Errors
/// Returns an error if the source isn't parseable TOML, has no `[router]`
/// section, or produces an invalid router config after reshaping.
pub(crate) fn caliban_toml_to_settings_router(body: &str) -> Result<serde_json::Value> {
    let root: toml::Value = toml::from_str(body).context("parsing source caliban.toml")?;
    let table = root
        .as_table()
        .context("source caliban.toml is not a TOML table")?;
    let mut router = table
        .get("router")
        .cloned()
        .context("source caliban.toml has no [router] section to migrate")?;
    if let Some(provider) = table.get("provider").cloned()
        && let Some(router_tbl) = router.as_table_mut()
    {
        // Top-level [provider.X] → [router.provider.X] under the settings key.
        router_tbl.insert("provider".to_string(), provider);
    }
    // Convert to the JSON shape `Settings.router` carries, then validate it
    // loads exactly as `wire_router` will.
    let json = serde_json::to_value(&router).context("converting router config to JSON")?;
    router_and_providers_from_value(&json)
        .map_err(|e| anyhow!("migrated router config is invalid: {e}"))?;
    Ok(json)
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
            let base_url_overridden = base_url.is_some();
            let make_cfg = move |key: secrecy::SecretString| -> Result<DirectConfig> {
                let mut cfg = DirectConfig::new(key);
                if let Some(url) = base_url.as_ref() {
                    cfg.base_url = url::Url::parse(url)?;
                }
                Ok(cfg)
            };
            // A local `base_url` override (llama.cpp, mlx-lm, …) needs no key —
            // tolerate an absent one there; canonical OpenAI still requires it (#641).
            let key = resolve_key_optional("openai", api_key_env, pool, base_url_overridden)?;
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
            "unknown provider '{other}' — supported: anthropic, openai, google"
        )),
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
        "add a [router] section to .caliban/settings.toml (or pass --config <PATH>) to enable \
         fallback / hedging / capability filtering (ADR 0038, ADR 0060).",
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
    settings_router: Option<&serde_json::Value>,
) -> Result<String> {
    use std::fmt::Write as _;
    // Router debug uses an empty helper pool — diagnostics shouldn't
    // spawn external scripts.
    let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
    let Some(wiring) = wire_router(settings_router, explicit_config, &empty_pool)? else {
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
    let source = if wiring.from_settings {
        "settings [router] section".to_string()
    } else {
        wiring.config_path.display().to_string()
    };
    let mut out = format!("config: {source}\n");
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

    const MINIMAL_ROUTE: &str = r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "openai"
model = "gpt-5.5"
fallback = []

[provider.openai]
base_url = "http://localhost:8080/v1"
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
        // The route points at a local `base_url`, so provider construction needs
        // no API key (#641) — no env setup required.
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("caliban.toml"), MINIMAL_ROUTE).unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let args = RouterDebugArgs {
            purpose: "main_loop".into(),
            has_vision: false,
            has_tools: false,
            has_thinking: false,
            effort: Some("high".into()),
        };
        let out = run_debug(&args, Some(&tmp.path().join("caliban.toml")), None).unwrap();
        assert!(out.contains("openai:gpt-5.5:main_loop"), "got:\n{out}");
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
        let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        let err =
            wire_router(None, Some(&tmp.path().join("caliban.toml")), &empty_pool).unwrap_err();
        let s = format!("{err:?}");
        assert!(s.contains("unknown provider"), "{s}");
    }

    #[test]
    fn no_settings_no_explicit_returns_none() {
        // #699: with discovery removed, no settings [router] and no --config
        // means the caller falls back to the single-provider path — a
        // caliban.toml sitting in the cwd is NOT auto-loaded.
        let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        let wiring = wire_router(None, None, &empty_pool).unwrap();
        assert!(wiring.is_none(), "no implicit discovery");
    }

    #[test]
    fn settings_router_is_used_when_no_explicit() {
        // #540/#699: a present settings [router] value is wired, and its nested
        // [router.provider.X] block is honored — the local base_url means no API
        // key is needed, which only works if the provider block was applied.
        let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        let value = serde_json::json!({
            "default_purpose": "main_loop",
            "route": [ { "purpose": "main_loop", "provider": "openai", "model": "x" } ],
            "provider": { "openai": { "base_url": "http://localhost:8080/v1" } }
        });
        let wiring = wire_router(Some(&value), None, &empty_pool)
            .unwrap()
            .expect("settings [router] should wire");
        assert!(wiring.from_settings, "settings-sourced wiring");
        assert_eq!(wiring.router.routes().len(), 1);
    }

    #[test]
    fn explicit_config_overrides_settings() {
        // #699: an explicit --config file wins over the settings [router].
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("caliban.toml"), MINIMAL_ROUTE).unwrap();
        let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        // A perfectly valid settings value that would otherwise be used.
        let settings = serde_json::json!({
            "default_purpose": "main_loop",
            "route": [ { "purpose": "main_loop", "provider": "openai", "model": "y" } ],
            "provider": { "openai": { "base_url": "http://localhost:9/v1" } }
        });
        let wiring = wire_router(
            Some(&settings),
            Some(&tmp.path().join("caliban.toml")),
            &empty_pool,
        )
        .unwrap()
        .expect("explicit --config should wire");
        assert!(
            !wiring.from_settings,
            "file-sourced wiring wins over settings"
        );
    }

    #[test]
    fn malformed_settings_router_errors() {
        // A present-but-invalid settings [router] is a hard error (no silent
        // fallback), when no explicit --config overrides it.
        let empty_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        let bad = serde_json::json!({ "route": [] }); // missing default_purpose
        let err = wire_router(Some(&bad), None, &empty_pool).unwrap_err();
        let s = format!("{err:?}");
        assert!(s.contains("[router] section from settings"), "{s}");
    }

    #[test]
    fn import_reshapes_provider_under_router() {
        // #699: `caliban config import-router` moves top-level [provider.X]
        // under router.provider.X and validates the result.
        let router = caliban_toml_to_settings_router(MINIMAL_ROUTE).unwrap();
        let provider = router
            .get("provider")
            .and_then(|p| p.get("openai"))
            .and_then(|o| o.get("base_url"))
            .and_then(|u| u.as_str());
        assert_eq!(provider, Some("http://localhost:8080/v1"));
        assert!(router.get("default_purpose").is_some());
    }

    #[test]
    fn import_rejects_source_without_router() {
        let err =
            caliban_toml_to_settings_router("[provider.openai]\nbase_url = \"x\"\n").unwrap_err();
        assert!(
            format!("{err:?}").contains("no [router] section"),
            "{err:?}"
        );
    }
}
