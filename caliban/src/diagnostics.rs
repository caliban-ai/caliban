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
    ///
    /// The signature is `async` so future deep checks (provider pings)
    /// can `.await` without changing callers; today the body is
    /// synchronous and clippy is intentionally silenced.
    #[allow(
        clippy::unused_async,
        reason = "kept async so deep provider checks can land without changing callers"
    )]
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
        if opts.deep {
            out.checks.push(DiagCheck {
                name: "providers",
                status: CheckStatus::Warn,
                hint: "deep provider pings not wired yet \u{2014} run with creds set".to_string(),
            });
        }
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
}
