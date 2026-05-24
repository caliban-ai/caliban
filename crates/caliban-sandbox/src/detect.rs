//! Backend detection — locate `sandbox-exec` / `bwrap` and probe versions.

use std::path::{Path, PathBuf};

use crate::config::Policy;
use crate::error::SandboxError;

/// Which OS sandbox backend will be used for this session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// macOS Seatbelt (`sandbox-exec`).
    Seatbelt {
        /// Resolved path to the `sandbox-exec` binary.
        path: PathBuf,
    },
    /// Linux / WSL bubblewrap (`bwrap`).
    Bwrap {
        /// Resolved path to the `bwrap` binary.
        path: PathBuf,
        /// Version string reported by `bwrap --version`.
        version: String,
    },
    /// Backend is not available on this host. The caller decides whether
    /// this is fatal (via `fail_if_unavailable`).
    Unavailable,
}

impl Backend {
    /// Short human-readable label, useful for logging.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Seatbelt { .. } => "seatbelt",
            Self::Bwrap { .. } => "bwrap",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Default Seatbelt path; overridable via `policy.sandbox_exec_path`.
#[cfg(target_os = "macos")]
const DEFAULT_SEATBELT_PATH: &str = "/usr/bin/sandbox-exec";

/// Minimum bubblewrap version we support (`--die-with-parent` arrived in 0.5).
#[cfg(target_os = "linux")]
const MIN_BWRAP_VERSION: (u32, u32) = (0, 5);

/// Detect the sandbox backend for the current host given a policy.
///
/// On macOS: probes `policy.sandbox_exec_path` or `/usr/bin/sandbox-exec`.
/// On Linux: probes `policy.bwrap_path` or searches `$PATH` for `bwrap`,
/// then runs `<bwrap> --version` to confirm `>= MIN_BWRAP_VERSION`.
/// On Windows: returns [`Backend::Unavailable`] (or errors when
/// `fail_if_unavailable: true`).
///
/// # Errors
///
/// Returns [`SandboxError::BackendUnavailable`] /
/// [`SandboxError::BackendTooOld`] /
/// [`SandboxError::UnsupportedPlatform`] when `fail_if_unavailable` is
/// set and the backend can't be resolved.
pub fn detect(policy: &Policy) -> Result<Backend, SandboxError> {
    #[cfg(target_os = "macos")]
    {
        detect_seatbelt(policy)
    }
    #[cfg(target_os = "linux")]
    {
        detect_bwrap(policy)
    }
    #[cfg(target_os = "windows")]
    {
        if policy.fail_if_unavailable {
            return Err(SandboxError::UnsupportedPlatform { os: "windows" });
        }
        tracing::warn!(
            "sandbox: Windows native is not supported in v1; running unsandboxed. \
             Use WSL for the Linux bubblewrap backend."
        );
        Ok(Backend::Unavailable)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        if policy.fail_if_unavailable {
            return Err(SandboxError::UnsupportedPlatform {
                os: std::env::consts::OS,
            });
        }
        Ok(Backend::Unavailable)
    }
}

#[cfg(target_os = "macos")]
fn detect_seatbelt(policy: &Policy) -> Result<Backend, SandboxError> {
    let candidate = policy
        .sandbox_exec_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SEATBELT_PATH));
    if path_is_executable(&candidate) {
        Ok(Backend::Seatbelt { path: candidate })
    } else if policy.fail_if_unavailable {
        Err(SandboxError::BackendUnavailable {
            backend: "sandbox-exec",
            looked_at: candidate,
        })
    } else {
        tracing::warn!(
            "sandbox: sandbox-exec not found at {}; running unsandboxed",
            candidate.display()
        );
        Ok(Backend::Unavailable)
    }
}

#[cfg(target_os = "linux")]
fn detect_bwrap(policy: &Policy) -> Result<Backend, SandboxError> {
    let candidate = policy.bwrap_path.clone().or_else(|| which_in_path("bwrap"));

    let Some(path) = candidate else {
        if policy.fail_if_unavailable {
            return Err(SandboxError::BackendUnavailable {
                backend: "bwrap",
                looked_at: PathBuf::from("bwrap"),
            });
        }
        tracing::warn!("sandbox: bwrap not found on PATH; running unsandboxed");
        return Ok(Backend::Unavailable);
    };

    if !path_is_executable(&path) {
        if policy.fail_if_unavailable {
            return Err(SandboxError::BackendUnavailable {
                backend: "bwrap",
                looked_at: path,
            });
        }
        tracing::warn!(
            "sandbox: bwrap at {} is not executable; running unsandboxed",
            path.display()
        );
        return Ok(Backend::Unavailable);
    }

    let version_output = match std::process::Command::new(&path).arg("--version").output() {
        Ok(o) => o,
        Err(e) => {
            if policy.fail_if_unavailable {
                return Err(SandboxError::BackendUnavailable {
                    backend: "bwrap",
                    looked_at: path,
                });
            }
            tracing::warn!(error = %e, "sandbox: failed to probe bwrap --version; running unsandboxed");
            return Ok(Backend::Unavailable);
        }
    };

    let version_str = String::from_utf8_lossy(&version_output.stdout).to_string();
    let parsed = parse_bwrap_version(&version_str);

    match parsed {
        Some(v) if v >= MIN_BWRAP_VERSION => Ok(Backend::Bwrap {
            path,
            version: version_str.trim().to_string(),
        }),
        Some(v) => {
            if policy.fail_if_unavailable {
                return Err(SandboxError::BackendTooOld {
                    backend: "bwrap",
                    found: format!("{}.{}", v.0, v.1),
                    need: format!("{}.{}", MIN_BWRAP_VERSION.0, MIN_BWRAP_VERSION.1),
                });
            }
            tracing::warn!(
                "sandbox: bwrap version {}.{} too old (need >= {}.{}); running unsandboxed",
                v.0,
                v.1,
                MIN_BWRAP_VERSION.0,
                MIN_BWRAP_VERSION.1,
            );
            Ok(Backend::Unavailable)
        }
        None => {
            if policy.fail_if_unavailable {
                return Err(SandboxError::BackendTooOld {
                    backend: "bwrap",
                    found: version_str,
                    need: format!("{}.{}", MIN_BWRAP_VERSION.0, MIN_BWRAP_VERSION.1),
                });
            }
            tracing::warn!(
                "sandbox: could not parse bwrap version from {:?}; running unsandboxed",
                version_str
            );
            Ok(Backend::Unavailable)
        }
    }
}

/// Parse the `MAJOR.MINOR` prefix out of a `bwrap --version` line.
///
/// Output looks like `bubblewrap 0.7.0\n` — we extract `0.7`.
#[cfg(any(target_os = "linux", test))]
#[must_use]
fn parse_bwrap_version(s: &str) -> Option<(u32, u32)> {
    // Find the first numeric token.
    for tok in s.split_whitespace() {
        let mut parts = tok.split('.');
        if let (Some(maj), Some(min)) = (parts.next(), parts.next())
            && let (Ok(maj_n), Ok(min_n)) = (maj.parse::<u32>(), min.parse::<u32>())
        {
            return Some((maj_n, min_n));
        }
    }
    None
}

/// Cross-platform stat-based executability check. Reads the file mode on
/// Unix; on other platforms, just checks existence.
fn path_is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(p) {
            Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Minimal $PATH search — returns the first existing entry.
#[cfg(target_os = "linux")]
fn which_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(name);
        if path_is_executable(&cand) {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bwrap_version_normal() {
        assert_eq!(parse_bwrap_version("bubblewrap 0.7.0\n"), Some((0, 7)));
        assert_eq!(parse_bwrap_version("bubblewrap 0.5"), Some((0, 5)));
        assert_eq!(parse_bwrap_version("0.4.2"), Some((0, 4)));
    }

    #[test]
    fn parse_bwrap_version_missing() {
        assert_eq!(parse_bwrap_version(""), None);
        assert_eq!(parse_bwrap_version("bubblewrap dev"), None);
    }

    #[test]
    fn backend_label() {
        assert_eq!(Backend::Seatbelt { path: "/x".into() }.label(), "seatbelt");
        assert_eq!(
            Backend::Bwrap {
                path: "/x".into(),
                version: "0.7".into(),
            }
            .label(),
            "bwrap"
        );
        assert_eq!(Backend::Unavailable.label(), "unavailable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_seatbelt_default_path() {
        // Every supported macOS ships /usr/bin/sandbox-exec.
        let p = Policy::default();
        let b = detect(&p).expect("detect");
        match b {
            Backend::Seatbelt { path } => {
                assert_eq!(path, PathBuf::from(DEFAULT_SEATBELT_PATH));
            }
            other => panic!("expected Seatbelt, got {other:?}"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_seatbelt_missing_fail_if_unavailable() {
        let p = Policy {
            sandbox_exec_path: Some(PathBuf::from("/nonexistent/sandbox-exec")),
            fail_if_unavailable: true,
            ..Policy::default()
        };
        let err = detect(&p).expect_err("missing binary should fail");
        assert!(matches!(err, SandboxError::BackendUnavailable { .. }));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_seatbelt_missing_noop() {
        let p = Policy {
            sandbox_exec_path: Some(PathBuf::from("/nonexistent/sandbox-exec")),
            fail_if_unavailable: false,
            ..Policy::default()
        };
        let b = detect(&p).expect("missing without fail_if_unavailable is OK");
        assert_eq!(b, Backend::Unavailable);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_bwrap_missing_fail_if_unavailable() {
        let p = Policy {
            bwrap_path: Some(PathBuf::from("/nonexistent/bwrap")),
            fail_if_unavailable: true,
            ..Policy::default()
        };
        let err = detect(&p).expect_err("missing binary should fail");
        assert!(matches!(err, SandboxError::BackendUnavailable { .. }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_bwrap_missing_noop() {
        let p = Policy {
            bwrap_path: Some(PathBuf::from("/nonexistent/bwrap")),
            fail_if_unavailable: false,
            ..Policy::default()
        };
        let b = detect(&p).expect("missing without fail_if_unavailable is OK");
        assert_eq!(b, Backend::Unavailable);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn detect_windows_unsupported_when_failing() {
        let p = Policy {
            fail_if_unavailable: true,
            ..Policy::default()
        };
        let err = detect(&p).expect_err("windows should error when fail_if_unavailable");
        assert!(matches!(
            err,
            SandboxError::UnsupportedPlatform { os: "windows" }
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn detect_windows_warning_noop() {
        let p = Policy::default();
        let b = detect(&p).expect("windows without fail_if_unavailable is no-op");
        assert_eq!(b, Backend::Unavailable);
    }
}
