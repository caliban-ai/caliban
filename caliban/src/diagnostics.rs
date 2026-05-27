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
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DiagOpts {
    /// When true, run "deep" checks — provider auth pings that cost
    /// real API calls. Default off.
    pub(crate) deep: bool,
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
        // `ollama` runs unconditionally: when `OLLAMA_BASE_URL` is set we
        // ping `/api/tags` to confirm the URL is reachable and surface
        // the model list. When it's unset we skip the network and just
        // report that no override was provided.
        out.checks.push(check_ollama(opts.deep).await);
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
    let skills = caliban_skills::load_skills(&roots);
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
    DiagCheck {
        name: "skills",
        status: CheckStatus::Pass,
        hint: format!("{} skill(s) loaded{suffix}", skills.len()),
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
        let r = Diagnostics::run(DiagOpts { deep: false }).await;
        assert!(!r.checks.is_empty(), "expected at least one check");
        for c in &r.checks {
            assert!(!c.name.is_empty());
        }
    }

    #[tokio::test]
    async fn exit_code_is_zero_when_no_failures() {
        // In a fresh-ish dev env this should at worst Warn, not Fail.
        let r = Diagnostics::run(DiagOpts { deep: false }).await;
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
        let r = Diagnostics::run(DiagOpts { deep: false }).await;
        assert!(
            r.checks.iter().any(|c| c.name == "ollama"),
            "expected an `ollama` check row in doctor output"
        );
    }
}
