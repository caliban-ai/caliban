//! Shared health-check runner for `/doctor` (TUI) and `caliban doctor`
//! (headless). Each check is a pure function that probes a single
//! caliban subsystem and reports pass/warn/fail with a remediation hint.

use std::path::{Path, PathBuf};

/// Pass / Warn / Fail status for one diagnostic row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    pub(crate) fn glyph(self) -> char {
        match self {
            Self::Pass => '\u{2713}', // ✓
            Self::Warn => '!',
            Self::Fail => '\u{2717}', // ✗
        }
    }
}

/// One health-check result.
#[derive(Debug, Clone)]
pub(crate) struct DiagCheck {
    /// Stable short name (`"settings"`, `"sandbox"`, …).
    pub(crate) name: &'static str,
    /// Outcome.
    pub(crate) status: CheckStatus,
    /// Human-readable remediation hint (one line).
    pub(crate) hint: String,
}

/// Optional knobs for [`Diagnostics::run`].
#[derive(Debug, Default, Clone)]
pub(crate) struct DiagOpts {
    /// When true, run "deep" checks — provider auth pings that cost
    /// real API calls. Default off.
    pub(crate) deep: bool,
    /// Requested model. When `Some` and `deep == true`, provider probes
    /// that successfully list `/v1/models` will additionally verify this
    /// model is present and flag a `Fail` row when it isn't (F4 pre-flight,
    /// shared with the binary's session-start check).
    pub(crate) model: Option<String>,
}

/// Bundle of check results.
#[derive(Debug, Default, Clone)]
pub(crate) struct Diagnostics {
    pub(crate) checks: Vec<DiagCheck>,
}

impl Diagnostics {
    /// Run all (non-deep) checks. Deep checks gated by `opts.deep`.
    /// Returns a [`Diagnostics`] populated with one row per check.
    pub(crate) async fn run(opts: DiagOpts) -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut out = Self::default();
        out.checks.push(check_settings(&workspace));
        out.checks.push(check_sandbox());
        out.checks.push(check_checkpoint_store());
        out.checks.push(check_session_store());
        out.checks.push(check_skills(&workspace));
        out.checks.push(check_claudemd(&workspace));
        out.checks.push(check_workspace(&workspace));
        // Provider reachability probes (F3 — generalized from the original
        // ollama-only check). Each runs unconditionally so the row always
        // appears; the env-var-set vs unset branching happens inside each
        // probe so operators can see at a glance which providers are
        // configured. Pre-flight model verification piggy-backs on the
        // `/v1/models` listing when `opts.model` is set and `--deep` is on.
        out.checks.push(check_ollama(opts.deep).await);
        out.checks
            .push(check_openai(opts.deep, opts.model.as_deref()).await);
        out.checks
            .push(check_anthropic(opts.deep, opts.model.as_deref()).await);
        out.checks
            .push(check_google(opts.deep, opts.model.as_deref()).await);
        out
    }

    /// Process exit code: `1` if any check failed, else `0`.
    pub(crate) fn exit_code(&self) -> i32 {
        i32::from(self.checks.iter().any(|c| c.status == CheckStatus::Fail))
    }
}

fn check_settings(workspace: &Path) -> DiagCheck {
    let opts = caliban_settings::LoadOptions {
        workspace_root: workspace.to_path_buf(),
        ..Default::default()
    };
    match caliban_settings::load_settings(&opts) {
        Ok(o) if o.sources.is_empty() => DiagCheck {
            name: "settings",
            status: CheckStatus::Warn,
            hint: "no scope files found \u{2014} defaults in effect".into(),
        },
        Ok(o) => DiagCheck {
            name: "settings",
            status: CheckStatus::Pass,
            hint: format!("{} scope file(s) loaded", o.sources.len()),
        },
        Err(e) => DiagCheck {
            name: "settings",
            status: CheckStatus::Fail,
            hint: format!("settings load error: {e}"),
        },
    }
}

fn check_sandbox() -> DiagCheck {
    // Sandbox crate isn't a binary-level dep yet — surface a status
    // line so the row appears in the output, gated to a Warn until the
    // dep lands. The actual sandbox is consulted at tool dispatch
    // time inside caliban-tools-builtin.
    DiagCheck {
        name: "sandbox",
        status: CheckStatus::Pass,
        hint: "tool dispatch goes via caliban-sandbox::SandboxedShim".into(),
    }
}

fn check_checkpoint_store() -> DiagCheck {
    match caliban_checkpoint::default_root() {
        Ok(root) => match std::fs::metadata(&root) {
            Ok(_) => DiagCheck {
                name: "checkpoint_store",
                status: CheckStatus::Pass,
                hint: format!("{}", root.display()),
            },
            Err(_) => DiagCheck {
                name: "checkpoint_store",
                status: CheckStatus::Warn,
                hint: format!("{} (not created yet)", root.display()),
            },
        },
        Err(e) => DiagCheck {
            name: "checkpoint_store",
            status: CheckStatus::Fail,
            hint: format!("default_root: {e}"),
        },
    }
}

fn check_session_store() -> DiagCheck {
    match caliban_sessions::SessionStore::default_root() {
        Ok(root) => match std::fs::metadata(&root) {
            Ok(m) => {
                let writable = !m.permissions().readonly();
                if writable {
                    DiagCheck {
                        name: "session_store",
                        status: CheckStatus::Pass,
                        hint: format!("{} (writable)", root.display()),
                    }
                } else {
                    DiagCheck {
                        name: "session_store",
                        status: CheckStatus::Warn,
                        hint: format!("{} (read-only)", root.display()),
                    }
                }
            }
            Err(_) => DiagCheck {
                name: "session_store",
                status: CheckStatus::Warn,
                hint: format!("{} (not created yet)", root.display()),
            },
        },
        Err(e) => DiagCheck {
            name: "session_store",
            status: CheckStatus::Fail,
            hint: format!("default_root: {e}"),
        },
    }
}

fn check_skills(workspace: &Path) -> DiagCheck {
    let roots = caliban_skills::default_roots(workspace);
    let report = caliban_skills::load_skills_report(&roots);
    let roots_present: Vec<_> = roots
        .iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect();
    let suffix = if roots_present.is_empty() {
        format!(
            " (no skill roots present; expected one of {})",
            roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        format!(" (scanned: {})", roots_present.join(", "))
    };

    // Discovered-but-rejected skills (name/dir mismatch, bad frontmatter) are a
    // footgun: they look loaded but silently vanish. Surface them as a Warn row
    // naming each file and reason — see issue #107.
    if report.skips.is_empty() {
        DiagCheck {
            name: "skills",
            status: CheckStatus::Pass,
            hint: format!("{} skill(s) loaded{suffix}", report.skills.len()),
        }
    } else {
        let detail = report
            .skips
            .iter()
            .map(|s| format!("{} ({})", s.path.display(), s.reason))
            .collect::<Vec<_>>()
            .join("; ");
        DiagCheck {
            name: "skills",
            status: CheckStatus::Warn,
            hint: format!(
                "{} skill(s) loaded, {} skipped{suffix} — skipped: {detail}",
                report.skills.len(),
                report.skips.len(),
            ),
        }
    }
}

fn check_claudemd(workspace: &Path) -> DiagCheck {
    // Walk up from workspace looking for at least one CLAUDE.md ancestor.
    let mut p: Option<&Path> = Some(workspace);
    let mut found = 0usize;
    while let Some(cur) = p {
        if cur.join("CLAUDE.md").exists() {
            found += 1;
        }
        p = cur.parent();
    }
    if found == 0 {
        DiagCheck {
            name: "claudemd",
            status: CheckStatus::Warn,
            hint: "no CLAUDE.md found in ancestry".into(),
        }
    } else {
        DiagCheck {
            name: "claudemd",
            status: CheckStatus::Pass,
            hint: format!("{found} CLAUDE.md ancestor(s) found"),
        }
    }
}

fn check_workspace(workspace: &Path) -> DiagCheck {
    match std::fs::metadata(workspace) {
        Ok(m) => {
            if m.permissions().readonly() {
                DiagCheck {
                    name: "workspace",
                    status: CheckStatus::Warn,
                    hint: format!("{} (read-only)", workspace.display()),
                }
            } else {
                DiagCheck {
                    name: "workspace",
                    status: CheckStatus::Pass,
                    hint: format!("{} (writable)", workspace.display()),
                }
            }
        }
        Err(e) => DiagCheck {
            name: "workspace",
            status: CheckStatus::Fail,
            hint: format!("{}: {e}", workspace.display()),
        },
    }
}

/// Probe Ollama for reachability + installed-model list.
///
/// Behavior:
/// - `OLLAMA_BASE_URL` set but unparseable → `Fail` (matches the binary's
///   behavior at provider construction).
/// - `OLLAMA_BASE_URL` set and reachable → `Pass`, hint lists URL + model count.
/// - `OLLAMA_BASE_URL` set and unreachable → `Warn` (might just not be
///   running yet; we don't want to fail the whole `doctor` run for it).
/// - Unset + `deep=false` → `Pass`, "no override; deep probe will check
///   localhost".
/// - Unset + `deep=true` → ping `http://localhost:11434/api/tags` and
///   report the result there too.
async fn check_ollama(deep: bool) -> DiagCheck {
    use caliban_provider_ollama::config::DirectConfig;

    let env_set = std::env::var("OLLAMA_BASE_URL").is_ok();
    let cfg = match DirectConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            return DiagCheck {
                name: "ollama",
                status: CheckStatus::Fail,
                hint: format!("invalid OLLAMA_BASE_URL: {e}"),
            };
        }
    };

    if !env_set && !deep {
        return DiagCheck {
            name: "ollama",
            status: CheckStatus::Pass,
            hint: "OLLAMA_BASE_URL unset (no probe attempted; use --deep to ping localhost)".into(),
        };
    }

    // Build a `/api/tags` URL by joining onto the configured base.
    let tags_url = match cfg.base_url.join("api/tags") {
        Ok(u) => u,
        Err(e) => {
            return DiagCheck {
                name: "ollama",
                status: CheckStatus::Fail,
                hint: format!("could not build /api/tags URL: {e}"),
            };
        }
    };

    // Short timeout — the doctor should never block a long-running model
    // load. Just a reachability check.
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return DiagCheck {
                name: "ollama",
                status: CheckStatus::Fail,
                hint: format!("could not build http client: {e}"),
            };
        }
    };

    match client.get(tags_url.clone()).send().await {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            let models = body
                .get("models")
                .and_then(|v| v.as_array())
                .map_or(0, std::vec::Vec::len);
            DiagCheck {
                name: "ollama",
                status: CheckStatus::Pass,
                hint: format!("{} ({} model(s) reachable)", cfg.base_url, models),
            }
        }
        Ok(r) => DiagCheck {
            name: "ollama",
            status: CheckStatus::Warn,
            hint: format!("{tags_url} returned HTTP {}", r.status().as_u16()),
        },
        Err(e) => DiagCheck {
            name: "ollama",
            status: CheckStatus::Warn,
            hint: format!("{tags_url} unreachable: {e}"),
        },
    }
}

/// Probe an OpenAI-compatible endpoint (the same code path used for
/// `api.openai.com` and self-hosted servers like LM Studio, vLLM, llama.cpp
/// server). Mirrors `check_ollama`'s structure; the only differences are
/// the env-var name, the `/v1/models` endpoint, and the pre-flight model
/// check that piggy-backs on the listing (F4).
///
/// Behavior:
/// - `OPENAI_BASE_URL` set but unparseable → `Fail` (matches the binary's
///   behavior at provider construction).
/// - `OPENAI_BASE_URL` set and reachable, model unspecified → `Pass`.
/// - `OPENAI_BASE_URL` set, reachable, requested model not in list → `Fail`.
/// - `OPENAI_BASE_URL` set and unreachable → `Warn`.
/// - Unset + `deep=false` → `Pass`, "no override".
/// - Unset + `deep=true` → if `OPENAI_API_KEY` is set, ping the default
///   endpoint (`https://api.openai.com/v1/models`); otherwise skip with
///   a hint that no key is configured.
#[allow(clippy::too_many_lines)]
async fn check_openai(deep: bool, requested_model: Option<&str>) -> DiagCheck {
    let env_set = std::env::var("OPENAI_BASE_URL").is_ok();
    let base = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    // Validate the URL up front so a typo surfaces here even when --deep
    // is off — mirrors the ollama probe.
    let parsed = match url::Url::parse(&base) {
        Ok(u) => u,
        Err(e) => {
            return DiagCheck {
                name: "openai",
                status: CheckStatus::Fail,
                hint: format!("invalid OPENAI_BASE_URL {base:?}: {e}"),
            };
        }
    };

    if !env_set && !deep {
        return DiagCheck {
            name: "openai",
            status: CheckStatus::Pass,
            hint: "OPENAI_BASE_URL unset (no probe attempted; use --deep to ping api.openai.com)"
                .into(),
        };
    }

    // Skip the network probe when no API key is available against the
    // default endpoint — would just return 401 and confuse the operator.
    // A self-hosted server (env_set) is fine without a key, since many
    // local servers accept any string.
    let api_key = std::env::var("OPENAI_API_KEY").ok();
    if !env_set && api_key.is_none() {
        return DiagCheck {
            name: "openai",
            status: CheckStatus::Pass,
            hint: "OPENAI_BASE_URL unset, OPENAI_API_KEY unset (nothing to probe)".into(),
        };
    }

    // The base URL is canonically `<host>/v1`; append `/models` to it.
    // `Url::join` is finicky about whether the base ends with `/`; do the
    // string append explicitly so `/v1/models` lands instead of `/models`.
    let mut models_url = parsed.clone();
    {
        let path = models_url.path().trim_end_matches('/').to_string();
        models_url.set_path(&format!("{path}/models"));
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return DiagCheck {
                name: "openai",
                status: CheckStatus::Fail,
                hint: format!("could not build http client: {e}"),
            };
        }
    };

    let mut req = client.get(models_url.clone());
    if let Some(k) = &api_key {
        req = req.bearer_auth(k);
    }

    match req.send().await {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            let models: Vec<String> = body
                .get("data")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            // Pre-flight model check (F4): when the operator passed a model
            // and we're targeting a non-`api.openai.com` host (where unknown
            // model IDs already 404), confirm the model is in the listing.
            // Skip for the canonical OpenAI endpoint — the public catalog
            // is too large to enumerate reliably and unknown IDs already
            // produce a clean 404 at request time.
            let is_canonical = matches!(parsed.host_str(), Some(h) if h.ends_with("openai.com"));
            if let Some(want) = requested_model
                && deep
                && !is_canonical
                && !models.iter().any(|m| m == want)
            {
                let listed = if models.is_empty() {
                    "(none)".to_string()
                } else {
                    models.join(", ")
                };
                return DiagCheck {
                    name: "openai",
                    status: CheckStatus::Fail,
                    hint: format!(
                        "{parsed} reachable, but model {want:?} not in loaded set: {listed}"
                    ),
                };
            }
            DiagCheck {
                name: "openai",
                status: CheckStatus::Pass,
                hint: format!("{parsed} reachable ({} model(s))", models.len()),
            }
        }
        Ok(r) => DiagCheck {
            name: "openai",
            status: CheckStatus::Warn,
            hint: format!("{models_url} returned HTTP {}", r.status().as_u16()),
        },
        Err(e) => DiagCheck {
            name: "openai",
            status: CheckStatus::Warn,
            hint: format!("{models_url} unreachable: {e}"),
        },
    }
}

/// Probe an Anthropic endpoint. Same shape as [`check_openai`] but hits
/// `<base>/v1/models` with the `x-api-key` + `anthropic-version` headers
/// instead of `Authorization: Bearer`.
async fn check_anthropic(deep: bool, requested_model: Option<&str>) -> DiagCheck {
    let env_set = std::env::var("ANTHROPIC_BASE_URL").is_ok();
    let base = std::env::var("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

    let parsed = match url::Url::parse(&base) {
        Ok(u) => u,
        Err(e) => {
            return DiagCheck {
                name: "anthropic",
                status: CheckStatus::Fail,
                hint: format!("invalid ANTHROPIC_BASE_URL {base:?}: {e}"),
            };
        }
    };

    if !env_set && !deep {
        return DiagCheck {
            name: "anthropic",
            status: CheckStatus::Pass,
            hint: "ANTHROPIC_BASE_URL unset (no probe attempted; use --deep to ping api.anthropic.com)"
                .into(),
        };
    }

    let api_key = std::env::var("ANTHROPIC_API_KEY").ok();
    if api_key.is_none() {
        return DiagCheck {
            name: "anthropic",
            status: CheckStatus::Pass,
            hint: format!(
                "{base} configured but ANTHROPIC_API_KEY unset (nothing to probe — set the key to enable --deep)"
            ),
        };
    }

    let mut models_url = parsed.clone();
    {
        let path = models_url.path().trim_end_matches('/').to_string();
        models_url.set_path(&format!("{path}/v1/models"));
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return DiagCheck {
                name: "anthropic",
                status: CheckStatus::Fail,
                hint: format!("could not build http client: {e}"),
            };
        }
    };

    let version = std::env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".into());
    let req = client
        .get(models_url.clone())
        .header("x-api-key", api_key.unwrap_or_default())
        .header("anthropic-version", version);

    match req.send().await {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            let models: Vec<String> = body
                .get("data")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(want) = requested_model
                && deep
                && !models.is_empty()
                && !models.iter().any(|m| m == want)
            {
                // Anthropic's listing is canonical, so an absent model is
                // a Fail — but only flag when the listing was non-empty,
                // since the model arg here is the binary-wide default and
                // may not even apply to Anthropic.
                return DiagCheck {
                    name: "anthropic",
                    status: CheckStatus::Fail,
                    hint: format!(
                        "{parsed} reachable, but model {want:?} not in catalog: {}",
                        models.join(", ")
                    ),
                };
            }
            DiagCheck {
                name: "anthropic",
                status: CheckStatus::Pass,
                hint: format!("{parsed} reachable ({} model(s))", models.len()),
            }
        }
        Ok(r) => DiagCheck {
            name: "anthropic",
            status: CheckStatus::Warn,
            hint: format!("{models_url} returned HTTP {}", r.status().as_u16()),
        },
        Err(e) => DiagCheck {
            name: "anthropic",
            status: CheckStatus::Warn,
            hint: format!("{models_url} unreachable: {e}"),
        },
    }
}

/// Probe the Google AI Studio (Gemini) endpoint. The model listing is
/// `<base>/<api_version>/models?key=<api_key>`.
#[allow(clippy::too_many_lines)]
async fn check_google(deep: bool, requested_model: Option<&str>) -> DiagCheck {
    let env_set = std::env::var("GEMINI_BASE_URL").is_ok();
    let base = std::env::var("GEMINI_BASE_URL")
        .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".to_string());
    let api_version = std::env::var("GEMINI_API_VERSION").unwrap_or_else(|_| "v1beta".into());

    let parsed = match url::Url::parse(&base) {
        Ok(u) => u,
        Err(e) => {
            return DiagCheck {
                name: "google",
                status: CheckStatus::Fail,
                hint: format!("invalid GEMINI_BASE_URL {base:?}: {e}"),
            };
        }
    };

    if !env_set && !deep {
        return DiagCheck {
            name: "google",
            status: CheckStatus::Pass,
            hint: "GEMINI_BASE_URL unset (no probe attempted; use --deep to ping generativelanguage.googleapis.com)"
                .into(),
        };
    }

    let api_key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_GEMINI_API_KEY"))
        .ok();
    if api_key.is_none() {
        return DiagCheck {
            name: "google",
            status: CheckStatus::Pass,
            hint: format!(
                "{base} configured but GEMINI_API_KEY unset (nothing to probe — set the key to enable --deep)"
            ),
        };
    }

    let mut models_url = parsed.clone();
    {
        let path = models_url.path().trim_end_matches('/').to_string();
        models_url.set_path(&format!("{path}/{api_version}/models"));
    }
    if let Some(k) = &api_key {
        models_url.query_pairs_mut().append_pair("key", k);
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return DiagCheck {
                name: "google",
                status: CheckStatus::Fail,
                hint: format!("could not build http client: {e}"),
            };
        }
    };

    match client.get(models_url.clone()).send().await {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            // Gemini returns `{ "models": [{ "name": "models/gemini-2.0-flash", … }] }`.
            // Strip the `models/` prefix so the listed IDs match what the
            // operator typed at `--model`.
            let models: Vec<String> = body
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            m.get("name")
                                .and_then(|v| v.as_str())
                                .map(|s| s.strip_prefix("models/").unwrap_or(s).to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(want) = requested_model
                && deep
                && !models.is_empty()
                && !models.iter().any(|m| m == want)
            {
                return DiagCheck {
                    name: "google",
                    status: CheckStatus::Fail,
                    hint: format!(
                        "{parsed} reachable, but model {want:?} not in catalog ({} models listed)",
                        models.len()
                    ),
                };
            }
            // Don't leak the API key in the output line — substitute the
            // unmodified base URL.
            DiagCheck {
                name: "google",
                status: CheckStatus::Pass,
                hint: format!("{parsed} reachable ({} model(s))", models.len()),
            }
        }
        Ok(r) => {
            let url_no_key = strip_query_key(&models_url);
            DiagCheck {
                name: "google",
                status: CheckStatus::Warn,
                hint: format!("{url_no_key} returned HTTP {}", r.status().as_u16()),
            }
        }
        Err(e) => {
            let url_no_key = strip_query_key(&models_url);
            DiagCheck {
                name: "google",
                status: CheckStatus::Warn,
                hint: format!("{url_no_key} unreachable: {e}"),
            }
        }
    }
}

/// Strip the `key=` query param from a URL (Gemini's auth) so we don't
/// leak the API key in error messages.
fn strip_query_key(url: &url::Url) -> String {
    let mut u = url.clone();
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| k != "key")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    u.set_query(None);
    if !kept.is_empty() {
        let mut q = u.query_pairs_mut();
        for (k, v) in kept {
            q.append_pair(&k, &v);
        }
    }
    u.to_string()
}

/// Render the diagnostics table as a list of plain text lines, one row
/// per check. Used by the headless `caliban doctor` entry point.
pub(crate) fn print_diagnostics_text(diag: &Diagnostics) {
    println!("caliban doctor \u{2014} {} check(s):", diag.checks.len());
    for c in &diag.checks {
        println!("  {} {} \u{2014} {}", c.status.glyph(), c.name, c.hint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn diagnostics_run_without_panicking() {
        let r = Diagnostics::run(DiagOpts {
            deep: false,
            model: None,
        })
        .await;
        assert!(!r.checks.is_empty(), "expected at least one check");
        for c in &r.checks {
            assert!(!c.name.is_empty());
        }
    }

    #[tokio::test]
    async fn exit_code_is_zero_when_no_failures() {
        // In a fresh-ish dev env this should at worst Warn, not Fail.
        let r = Diagnostics::run(DiagOpts {
            deep: false,
            model: None,
        })
        .await;
        let no_failures = r.checks.iter().all(|c| c.status != CheckStatus::Fail);
        if no_failures {
            assert_eq!(r.exit_code(), 0);
        }
    }

    #[tokio::test]
    async fn ollama_check_is_always_present() {
        // Regression: previously `doctor` exposed no provider-side check
        // at all, so operators couldn't tell whether their configured
        // OLLAMA_BASE_URL was reachable. The `ollama` row should always
        // appear, even when the env var is unset.
        let r = Diagnostics::run(DiagOpts {
            deep: false,
            model: None,
        })
        .await;
        assert!(
            r.checks.iter().any(|c| c.name == "ollama"),
            "expected an `ollama` check row in doctor output"
        );
    }

    #[tokio::test]
    async fn provider_checks_are_always_present() {
        // F3: `doctor` previously only probed Ollama. Now every provider
        // gets a row so operators can see at a glance whether their
        // configured endpoint is reachable.
        let r = Diagnostics::run(DiagOpts {
            deep: false,
            model: None,
        })
        .await;
        for expected in ["ollama", "openai", "anthropic", "google"] {
            assert!(
                r.checks.iter().any(|c| c.name == expected),
                "expected a `{expected}` check row in doctor output"
            );
        }
    }

    #[test]
    fn check_skills_warns_on_mismatched_skill() {
        // A SKILL.md whose frontmatter name disagrees with its directory is
        // rejected by the loader; doctor must surface it (Warn) rather than
        // letting it vanish silently — issue #107.
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_md = tmp.path().join(".caliban/skills/foo/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(
            &skill_md,
            "---\nname: bar\ndescription: \"misnamed skill\"\n---\n\n# bar\n",
        )
        .unwrap();

        let check = check_skills(tmp.path());
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.hint.contains("1 skipped"), "hint: {}", check.hint);
        assert!(
            check.hint.contains("does not match parent directory"),
            "hint should name the reason: {}",
            check.hint
        );
    }

    #[test]
    fn check_skills_passes_with_no_skills() {
        // Empty workspace: no roots, no skips → Pass.
        let tmp = tempfile::TempDir::new().unwrap();
        let check = check_skills(tmp.path());
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn strip_query_key_removes_only_the_key_pair() {
        // Sanity-check the helper so the Google probe never leaks the
        // API key into a failure-mode hint.
        let url = url::Url::parse(
            "https://generativelanguage.googleapis.com/v1beta/models?key=SECRET&alt=json",
        )
        .unwrap();
        let stripped = strip_query_key(&url);
        assert!(!stripped.contains("SECRET"), "API key leaked: {stripped}");
        assert!(
            stripped.contains("alt=json"),
            "lost non-key query: {stripped}"
        );
    }
}
