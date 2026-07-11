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
        // Present + supported version is necessary but not sufficient: the
        // runtime may install bwrap yet forbid unprivileged user namespaces
        // (#345). Probe an actual userns before committing to the sandbox.
        Some(v) if v >= MIN_BWRAP_VERSION => finalize_bwrap(path, &version_str, policy),
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

/// Argv (excluding the `bwrap` binary) for a minimal user-namespace probe:
/// create a user namespace over a read-only view of `/` and run `/bin/true`.
/// Succeeds iff the runtime permits `--unshare-user`.
#[cfg(any(target_os = "linux", test))]
fn userns_probe_args() -> Vec<std::ffi::OsString> {
    ["--ro-bind", "/", "/", "--unshare-user", "--", "/bin/true"]
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Decide userns availability from the probe process's exit code.
///
/// `bwrap` sets up **all** namespaces (including `--unshare-user`) *before* it
/// execs the final payload, so:
/// - `Some(0)`   — namespace created and `/bin/true` ran → available.
/// - `Some(127)` — namespace created, but the payload binary was absent on this
///   host (e.g. NixOS ships no `/bin/true`); the exec-stage failure still proves
///   userns works → available (#404). Previously this was misread as "denied",
///   silently downgrading a capable host to unsandboxed.
/// - any other code (e.g. `Some(1)` from a failed `--unshare-user`) or `None`
///   (signal) — the namespace could not be created → unavailable.
#[cfg(any(target_os = "linux", test))]
fn interpret_userns_probe(code: Option<i32>) -> bool {
    matches!(code, Some(0 | 127))
}

/// Run the userns probe with `bwrap` at `path`; `true` iff it can create a
/// user namespace on this host. A failure to even launch `bwrap` (the caller
/// already confirmed the binary exists, so this is a transient/permission
/// error) is treated as unavailable rather than as a userns denial.
#[cfg(any(target_os = "linux", test))]
fn probe_userns(path: &Path) -> bool {
    // ETXTBSY (errno 26 on Linux/macOS): exec of a binary that is still open for
    // writing fails spuriously. This bites a just-written binary — notably the
    // freshly-created fake `bwrap` in tests, which flaked CI intermittently
    // (#441), but it can affect any recently-installed binary. Retry a few times
    // before concluding userns is unavailable; a spurious exec failure must not
    // silently drop the sandbox.
    const ETXTBSY: i32 = 26;
    for attempt in 0..5 {
        match std::process::Command::new(path)
            .args(userns_probe_args())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(s) => return interpret_userns_probe(s.code()),
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && attempt < 4 => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return false,
        }
    }
    false
}

/// Given a present, version-OK `bwrap`, confirm the runtime actually permits
/// user namespaces before committing to the sandbox. On denial, degrade like
/// any other unavailable backend (or error under `fail_if_unavailable`).
///
/// Skipped when `enable_weaker_nested_sandbox` is set — that mode deliberately
/// omits `--unshare-user`, so userns availability is irrelevant.
#[cfg(any(target_os = "linux", test))]
fn finalize_bwrap(
    path: PathBuf,
    version_str: &str,
    policy: &Policy,
) -> Result<Backend, SandboxError> {
    if !policy.enable_weaker_nested_sandbox && !probe_userns(&path) {
        if policy.fail_if_unavailable {
            return Err(SandboxError::BackendUnavailable {
                backend: "bwrap",
                looked_at: path,
            });
        }
        tracing::warn!(
            "sandbox: bwrap at {} cannot create a user namespace \
             (runtime denies unprivileged userns); running unsandboxed",
            path.display()
        );
        return Ok(Backend::Unavailable);
    }
    Ok(Backend::Bwrap {
        path,
        version: version_str.trim().to_string(),
    })
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

    // ─── userns probe + graceful fallback (#345) ────────────────────────────
    // Deterministic on any unix host via a fake `bwrap`: `--version` reports a
    // supported version; the probe invocation exits with `probe_exit`.
    #[cfg(unix)]
    fn fake_bwrap(dir: &tempfile::TempDir, probe_exit: u8) -> PathBuf {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.path().join("bwrap");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(
            f,
            "if [ \"$1\" = \"--version\" ]; then echo 'bubblewrap 0.7.0'; exit 0; fi"
        )
        .unwrap();
        writeln!(f, "exit {probe_exit}").unwrap();
        drop(f);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn userns_probe_args_unshares_user_over_readonly_root() {
        let args = userns_probe_args();
        assert!(args.iter().any(|a| a == "--unshare-user"));
        assert!(args.iter().any(|a| a == "--ro-bind"));
    }

    #[test]
    fn interpret_userns_probe_treats_missing_payload_as_available() {
        assert!(interpret_userns_probe(Some(0)), "ns created, payload ran");
        assert!(
            interpret_userns_probe(Some(127)),
            "ns created, payload absent (e.g. NixOS, no /bin/true) ⇒ available (#404)"
        );
        assert!(
            !interpret_userns_probe(Some(1)),
            "--unshare-user setup failed ⇒ denied"
        );
        assert!(
            !interpret_userns_probe(None),
            "killed by signal ⇒ not available"
        );
    }

    #[cfg(unix)]
    #[test]
    fn probe_userns_reflects_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            probe_userns(&fake_bwrap(&dir, 0)),
            "exit 0 ⇒ userns available"
        );
        // Exit 127: namespace was created but the payload binary was missing —
        // bwrap only reaches exec after userns setup, so this still means the
        // host supports userns (#404).
        let dir127 = tempfile::tempdir().unwrap();
        assert!(
            probe_userns(&fake_bwrap(&dir127, 127)),
            "exit 127 (payload absent) ⇒ userns still available"
        );
        let dir2 = tempfile::tempdir().unwrap();
        assert!(
            !probe_userns(&fake_bwrap(&dir2, 1)),
            "exit 1 (userns setup failed) ⇒ denied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn finalize_bwrap_returns_bwrap_when_userns_permitted() {
        let dir = tempfile::tempdir().unwrap();
        let path = fake_bwrap(&dir, 0);
        let b = finalize_bwrap(path.clone(), "bubblewrap 0.7.0\n", &Policy::default())
            .expect("userns permitted ⇒ Bwrap");
        assert_eq!(
            b,
            Backend::Bwrap {
                path,
                version: "bubblewrap 0.7.0".into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn finalize_bwrap_falls_back_when_userns_denied() {
        let dir = tempfile::tempdir().unwrap();
        let p = Policy {
            fail_if_unavailable: false,
            ..Policy::default()
        };
        let b = finalize_bwrap(fake_bwrap(&dir, 1), "bubblewrap 0.7.0\n", &p)
            .expect("userns denied without fail_if_unavailable ⇒ unsandboxed");
        assert_eq!(b, Backend::Unavailable);
    }

    #[cfg(unix)]
    #[test]
    fn finalize_bwrap_errors_when_userns_denied_and_strict() {
        let dir = tempfile::tempdir().unwrap();
        let p = Policy {
            fail_if_unavailable: true,
            ..Policy::default()
        };
        let err = finalize_bwrap(fake_bwrap(&dir, 1), "bubblewrap 0.7.0\n", &p)
            .expect_err("userns denied under fail_if_unavailable ⇒ error");
        assert!(matches!(err, SandboxError::BackendUnavailable { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn finalize_bwrap_skips_probe_in_nested_sandbox_mode() {
        // enable_weaker_nested_sandbox omits --unshare-user, so a denied probe
        // must not force a fallback.
        let dir = tempfile::tempdir().unwrap();
        let p = Policy {
            enable_weaker_nested_sandbox: true,
            ..Policy::default()
        };
        let path = fake_bwrap(&dir, 1);
        let b = finalize_bwrap(path.clone(), "bubblewrap 0.7.0\n", &p)
            .expect("nested mode ⇒ Bwrap regardless of userns probe");
        assert!(matches!(b, Backend::Bwrap { .. }));
    }
}
