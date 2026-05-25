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
/// provider path.
pub(crate) fn try_load(explicit: Option<&Path>, start_dir: &Path) -> Result<Option<RouterWiring>> {
    let Some(discovered) =
        discover_caliban_toml(explicit, start_dir).context("loading caliban.toml")?
    else {
        return Ok(None);
    };
    let DiscoveredConfig { path, config } = discovered;
    let Some(router_cfg) = config.router.clone() else {
        // caliban.toml exists but doesn't define [router].
        return Ok(None);
    };
    let providers = build_provider_handles(&router_cfg, &config.providers)?;
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
        let handle =
            build_one(name, &block).with_context(|| format!("constructing provider '{name}'"))?;
        out.insert(name.to_string(), handle);
    }
    Ok(out)
}

fn build_one(name: &str, block: &ProviderBlock) -> Result<Arc<dyn Provider + Send + Sync>> {
    match name {
        "anthropic" => {
            use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
            let api_key_env = block.api_key_env.as_deref().unwrap_or("ANTHROPIC_API_KEY");
            let key = std::env::var(api_key_env)
                .with_context(|| format!("env var {api_key_env} is unset"))?;
            let mut cfg = DirectConfig::new(secrecy::SecretString::from(key));
            if let Some(url) = block.base_url.as_ref() {
                cfg.base_url = url::Url::parse(url)?;
            }
            Ok(Arc::new(AnthropicProvider::direct(cfg)?))
        }
        "openai" => {
            use caliban_provider_openai::{OpenAIProvider, config::DirectConfig};
            let api_key_env = block.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
            let key = std::env::var(api_key_env)
                .with_context(|| format!("env var {api_key_env} is unset"))?;
            let mut cfg = DirectConfig::new(secrecy::SecretString::from(key));
            if let Some(url) = block.base_url.as_ref() {
                cfg.base_url = url::Url::parse(url)?;
            }
            Ok(Arc::new(OpenAIProvider::direct(cfg)?))
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
            let key = std::env::var(api_key_env)
                .with_context(|| format!("env var {api_key_env} is unset"))?;
            let cfg = AIStudioConfig::new(secrecy::SecretString::from(key));
            // base_url override is provider-specific; ignored for AI Studio's
            // fixed endpoint in v2 (operator can pin via env vars).
            let _ = block.base_url;
            Ok(Arc::new(GoogleProvider::ai_studio(cfg)?))
        }
        other => Err(anyhow!(
            "unknown provider '{other}' — supported: anthropic, openai, ollama, google"
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

/// Execute `caliban router debug` — print the resolved candidate list for a
/// synthetic request matching the CLI flags.
pub(crate) fn run_debug(
    args: &RouterDebugArgs,
    explicit_config: Option<&Path>,
    start_dir: &Path,
) -> Result<String> {
    use std::fmt::Write as _;
    let wiring = try_load(explicit_config, start_dir)?
        .ok_or_else(|| anyhow!("no caliban.toml found (router unconfigured)"))?;

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
        thinking: None,
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
        req.thinking = Some(caliban_provider::ThinkingConfig {
            budget_tokens: 4096,
        });
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
        let err = try_load(Some(&tmp.path().join("caliban.toml")), tmp.path()).unwrap_err();
        let s = format!("{err:?}");
        assert!(s.contains("unknown provider"), "{s}");
    }
}
