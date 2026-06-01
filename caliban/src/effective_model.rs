//! `EffectiveModel` — the resolved provider/model pair the binary
//! actually runs against. Built once in `main.rs` from the CLI args
//! and the merged `Settings` snapshot; threaded into every site that
//! previously called `default_model_for(args.provider)`.
//!
//! Precedence (high → low):
//!
//! 1. `--provider` and `--model` CLI flags (each independently, when
//!    explicitly set).
//! 2. `Settings.model` (qualified form pins provider; bare-name keeps
//!    the CLI/default provider with a warning).
//! 3. Builtin default: Anthropic + `default_model_for(Anthropic)`.

use anyhow::Result;
use caliban_settings::{ModelSelector, Settings};

use crate::args::{Args, ProviderKind, default_model_for};

/// Where the effective model selection came from. Surfaced in the
/// `/config` overlay and `caliban doctor` diagnostics so an operator
/// can see which precedence layer won.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelSource {
    /// One or both of `--provider` / `--model` won.
    Cli,
    /// `Settings.model` provided provider and/or name.
    Settings,
    /// Neither CLI nor Settings supplied anything — fell back to the
    /// hard-coded Anthropic default.
    BuiltinDefault,
}

impl ModelSource {
    /// Short label for the `/config` overlay.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Settings => "settings",
            Self::BuiltinDefault => "builtin-default",
        }
    }
}

/// Resolved provider/model pair for this run.
#[derive(Debug, Clone)]
pub(crate) struct EffectiveModel {
    /// Provider adapter to construct.
    pub provider: ProviderKind,
    /// Model name to send to the provider.
    pub name: String,
    /// Fallback `(provider, model)` used by the model router when the
    /// primary errors. `None` if no fallback configured.
    pub fallback: Option<(ProviderKind, String)>,
    /// Provenance for diagnostics.
    pub source: ModelSource,
}

impl EffectiveModel {
    /// Resolve CLI > Settings > Built-in.
    pub(crate) fn resolve(args: &Args, settings: &Settings) -> Result<Self> {
        let cli_provider = args.provider;
        let cli_model = args.model.as_deref();

        let (settings_provider, settings_name) = match &settings.model {
            Some(ModelSelector::Qualified { provider, name }) => {
                (Some(parse_provider(provider)?), Some(name.clone()))
            }
            Some(ModelSelector::Name(name)) => (None, Some(name.clone())),
            None => (None, None),
        };

        let provider = cli_provider
            .or(settings_provider)
            .unwrap_or(ProviderKind::Anthropic);

        let (name, source) = if let Some(m) = cli_model {
            (
                m.to_string(),
                if cli_provider.is_some() || settings_name.is_none() {
                    ModelSource::Cli
                } else {
                    // CLI model only, settings provided a name we ignored.
                    ModelSource::Cli
                },
            )
        } else if let Some(n) = settings_name.clone() {
            (n, ModelSource::Settings)
        } else if cli_provider.is_some() {
            (default_model_for(provider).to_string(), ModelSource::Cli)
        } else {
            (
                default_model_for(provider).to_string(),
                ModelSource::BuiltinDefault,
            )
        };

        if settings_name.is_some() && settings_provider.is_none() && cli_provider.is_none() {
            tracing::warn!(
                target: "caliban::config",
                model = %name,
                "[model] bare-string in settings has no provider; defaulting to anthropic — \
                 pin via `[model] provider = \"...\"` to avoid this warning",
            );
        }

        Ok(Self {
            provider,
            name,
            fallback: fallback_from_settings(settings)?,
            source,
        })
    }
}

fn fallback_from_settings(settings: &Settings) -> Result<Option<(ProviderKind, String)>> {
    match &settings.fallback_model {
        Some(ModelSelector::Qualified { provider, name }) => {
            Ok(Some((parse_provider(provider)?, name.clone())))
        }
        Some(ModelSelector::Name(name)) => Ok(Some((ProviderKind::Anthropic, name.clone()))),
        None => Ok(None),
    }
}

fn parse_provider(s: &str) -> Result<ProviderKind> {
    match s {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "openai" => Ok(ProviderKind::Openai),
        "ollama" => Ok(ProviderKind::Ollama),
        "google" => Ok(ProviderKind::Google),
        other => anyhow::bail!("unknown provider in settings: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_settings::ModelSelector;
    use clap::Parser;

    fn args(provider: Option<ProviderKind>, model: Option<&str>) -> Args {
        let mut argv: Vec<String> = vec!["caliban".into()];
        if let Some(p) = provider {
            argv.push("--provider".into());
            argv.push(
                match p {
                    ProviderKind::Anthropic => "anthropic",
                    ProviderKind::Openai => "openai",
                    ProviderKind::Ollama => "ollama",
                    ProviderKind::Google => "google",
                }
                .into(),
            );
        }
        if let Some(m) = model {
            argv.push("--model".into());
            argv.push(m.into());
        }
        Args::try_parse_from(argv).expect("parse args")
    }

    fn settings_with_model(provider: &str, name: &str) -> Settings {
        Settings {
            model: Some(ModelSelector::Qualified {
                provider: provider.into(),
                name: name.into(),
            }),
            ..Settings::default()
        }
    }

    #[test]
    fn cli_provider_and_model_win_over_settings() {
        let s = settings_with_model("openai", "gpt-4o");
        let eff = EffectiveModel::resolve(
            &args(Some(ProviderKind::Anthropic), Some("claude-haiku-4-7")),
            &s,
        )
        .unwrap();
        assert!(matches!(eff.provider, ProviderKind::Anthropic));
        assert_eq!(eff.name, "claude-haiku-4-7");
        assert_eq!(eff.source, ModelSource::Cli);
    }

    #[test]
    fn settings_qualified_picks_provider_and_model() {
        let s = settings_with_model("openai", "gpt-4o");
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Openai));
        assert_eq!(eff.name, "gpt-4o");
        assert_eq!(eff.source, ModelSource::Settings);
    }

    #[test]
    fn settings_bare_name_keeps_anthropic_default() {
        let s = Settings {
            model: Some(ModelSelector::Name("gpt-4o".into())),
            ..Settings::default()
        };
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Anthropic));
        assert_eq!(eff.name, "gpt-4o");
        assert_eq!(eff.source, ModelSource::Settings);
    }

    #[test]
    fn cli_model_only_takes_settings_provider() {
        let s = settings_with_model("openai", "gpt-4o");
        let eff = EffectiveModel::resolve(&args(None, Some("gpt-5.5")), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Openai));
        assert_eq!(eff.name, "gpt-5.5");
        assert_eq!(eff.source, ModelSource::Cli);
    }

    #[test]
    fn cli_provider_only_uses_default_model_for_that_provider() {
        let s = Settings::default();
        let eff = EffectiveModel::resolve(&args(Some(ProviderKind::Openai), None), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Openai));
        assert_eq!(eff.name, default_model_for(ProviderKind::Openai));
        assert_eq!(eff.source, ModelSource::Cli);
    }

    #[test]
    fn nothing_set_falls_back_to_builtin_default() {
        let s = Settings::default();
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Anthropic));
        assert_eq!(eff.name, default_model_for(ProviderKind::Anthropic));
        assert_eq!(eff.source, ModelSource::BuiltinDefault);
    }

    #[test]
    fn fallback_qualified_lifts_from_settings() {
        let s = Settings {
            fallback_model: Some(ModelSelector::Qualified {
                provider: "anthropic".into(),
                name: "claude-haiku-4-7".into(),
            }),
            ..Settings::default()
        };
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        assert_eq!(
            eff.fallback,
            Some((ProviderKind::Anthropic, "claude-haiku-4-7".into())),
        );
    }

    #[test]
    fn unknown_provider_in_settings_errors() {
        let s = Settings {
            model: Some(ModelSelector::Qualified {
                provider: "totally-made-up".into(),
                name: "foo".into(),
            }),
            ..Settings::default()
        };
        let err = EffectiveModel::resolve(&args(None, None), &s).unwrap_err();
        assert!(err.to_string().contains("totally-made-up"));
    }
}
