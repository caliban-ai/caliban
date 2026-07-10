//! The runtime shim: wrap a `tokio::process::Command` in a sandboxed
//! equivalent (`sandbox-exec` on macOS, `bwrap` on Linux).
//!
//! `wrap_command` is the entry point invoked by `BashTool` before it
//! spawns. The shim composes with the existing process-group cleanup
//! logic — both `sandbox-exec` and `bwrap` propagate signals to their
//! child, so the bash tool's existing `kill_process_tree(pid)` still
//! reaps the whole tree.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Command as TokioCommand;

use crate::bwrap;
use crate::config::Policy;
use crate::detect::{Backend, detect};
use crate::error::SandboxError;
use crate::seatbelt;

/// Session-lifetime sandbox state. Holds the resolved backend + policy
/// plus, on macOS, the on-disk profile path.
#[derive(Debug)]
pub struct SandboxedShim {
    backend: Backend,
    policy: Policy,
    /// Holds the temp file alive for the lifetime of the shim. `None`
    /// on Linux / when sandbox disabled.
    _seatbelt_profile: Option<tempfile::NamedTempFile>,
    /// Path to the rendered Seatbelt profile (or `None`).
    seatbelt_profile_path: Option<PathBuf>,
}

impl SandboxedShim {
    /// Construct a shim from a policy. On macOS, renders the Seatbelt
    /// profile and writes it to a `$TMPDIR` file with mode `0600`.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError`] when the backend can't be resolved (and
    /// `fail_if_unavailable: true`), the policy is invalid, or the
    /// profile can't be written.
    pub fn new(policy: Policy) -> Result<Self, SandboxError> {
        // Sanity check on conflicting flags before talking to the OS.
        validate_policy(&policy)?;

        if !policy.enabled {
            return Ok(Self {
                backend: Backend::Unavailable,
                policy,
                _seatbelt_profile: None,
                seatbelt_profile_path: None,
            });
        }

        let backend = detect(&policy)?;

        // (Previously a warn here about allowed_domains-without-proxy on Linux;
        // that configuration is now rejected outright by `validate_policy`
        // above (#403), so the case is unreachable.)

        let (profile_temp, profile_path) = match &backend {
            Backend::Seatbelt { .. } => {
                let text = seatbelt::render_profile(&policy);
                let tf = write_profile(&text)?;
                let path = tf.path().to_path_buf();
                (Some(tf), Some(path))
            }
            _ => (None, None),
        };

        Ok(Self {
            backend,
            policy,
            _seatbelt_profile: profile_temp,
            seatbelt_profile_path: profile_path,
        })
    }

    /// The resolved backend (informational; useful for `--debug`).
    #[must_use]
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// The policy backing this shim.
    #[must_use]
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Returns `true` when the shim will actually wrap (vs. no-op).
    /// Operators / observers use this to gate the
    /// `auto_allow_bash_if_sandboxed` short-circuit.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.policy.enabled && !matches!(self.backend, Backend::Unavailable)
    }

    /// Returns `true` when the sandbox should auto-allow Bash without
    /// asking — i.e. policy is active AND the operator opted in.
    #[must_use]
    pub fn auto_allows_bash(&self) -> bool {
        self.is_active() && self.policy.auto_allow_bash_if_sandboxed
    }

    /// Returns `true` when the command should bypass the sandbox per the
    /// `allow_unsandboxed_commands` list. Exposed so the permission
    /// layer can decide whether to still apply `auto_allow_bash`.
    #[must_use]
    pub fn is_unsandboxed_command(&self, command_str: &str) -> bool {
        self.policy.is_unsandboxed_command(command_str)
    }

    /// Where the rendered Seatbelt profile lives on disk (None on Linux).
    /// Useful for `--debug` and integration tests.
    #[must_use]
    pub fn seatbelt_profile_path(&self) -> Option<&std::path::Path> {
        self.seatbelt_profile_path.as_deref()
    }

    /// Mutate `cmd` in place: replace its program/args with a sandboxed
    /// wrapper invocation. `command_str` is the original shell command
    /// (used to check `allow_unsandboxed_commands`).
    ///
    /// No-op when the shim is inactive or the command is on the bypass
    /// list. Preserves `current_dir`, env vars, stdio plumbing, and any
    /// flags the caller already set (`kill_on_drop`, `process_group`).
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::InvalidConfig`] only if a path inside the
    /// policy fails to convert to an OS string (very rare).
    pub fn wrap_command(
        &self,
        cmd: &mut TokioCommand,
        command_str: &str,
    ) -> Result<(), SandboxError> {
        if !self.is_active() || self.is_unsandboxed_command(command_str) {
            return Ok(());
        }

        match &self.backend {
            Backend::Seatbelt { path } => {
                let Some(profile_path) = self.seatbelt_profile_path.as_deref() else {
                    return Err(SandboxError::InvalidConfig {
                        reason: "Seatbelt backend without rendered profile".into(),
                    });
                };
                wrap_with_program(
                    cmd,
                    path.as_os_str().to_owned(),
                    vec![
                        "-f".into(),
                        profile_path.as_os_str().to_owned(),
                        "--".into(),
                    ],
                );
            }
            Backend::Bwrap { path, .. } => {
                let mut bwrap_args = bwrap::build_args(&self.policy);
                bwrap_args.push("--".into());
                wrap_with_program(cmd, path.as_os_str().to_owned(), bwrap_args);
            }
            Backend::Unavailable => {
                // is_active() guards this branch, but match exhaustively.
            }
        }

        Ok(())
    }
}

/// Build a Seatbelt profile file under `$TMPDIR` with mode 0600.
/// Returns the `NamedTempFile` so the caller can keep it alive for the
/// session.
fn write_profile(text: &str) -> Result<tempfile::NamedTempFile, SandboxError> {
    use std::io::Write;

    let dir = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);

    let tf = tempfile::Builder::new()
        .prefix("caliban-sandbox-")
        .suffix(".sb")
        .tempfile_in(&dir)
        .map_err(|e| SandboxError::PolicyWrite {
            path: dir.clone(),
            source: e,
        })?;

    // Apply mode 0600 on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(tf.path(), perms).map_err(|e| SandboxError::PolicyWrite {
            path: tf.path().to_path_buf(),
            source: e,
        })?;
    }

    // Write the contents.
    {
        let mut f = tf.reopen().map_err(|e| SandboxError::PolicyWrite {
            path: tf.path().to_path_buf(),
            source: e,
        })?;
        f.write_all(text.as_bytes())
            .map_err(|e| SandboxError::PolicyWrite {
                path: tf.path().to_path_buf(),
                source: e,
            })?;
    }

    Ok(tf)
}

/// Replace the program / args of an existing `tokio::process::Command`
/// with a wrapper invocation.
///
/// We can't directly mutate the inner program of an existing
/// `tokio::process::Command`, so we rebuild it on top of the same
/// stdio / env / cwd flags by taking ownership of the std command,
/// then write the rebuilt command back into `*cmd`. The caller's
/// `kill_on_drop` / `process_group` / stdio settings are re-applied
/// to the new command to match the `BashTool`'s expectations.
fn wrap_with_program(cmd: &mut TokioCommand, program: OsString, prefix_args: Vec<OsString>) {
    // Snapshot the existing command. `as_std()` gives us a reference to
    // the underlying `std::process::Command` from which we can extract
    // program/args/env/cwd; we then rebuild a fresh Tokio command on
    // top of the new program.
    let std_cmd = cmd.as_std();
    let original_program = std_cmd.get_program().to_owned();
    let original_args: Vec<OsString> = std_cmd.get_args().map(std::ffi::OsStr::to_owned).collect();
    let cwd = std_cmd.get_current_dir().map(std::path::Path::to_path_buf);
    let envs: Vec<(OsString, Option<OsString>)> = std_cmd
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(std::ffi::OsStr::to_owned)))
        .collect();
    let env_clear = std_cmd.get_envs().any(|(_, v)| v.is_none())
        && std_cmd.get_envs().count() == envs.len()
        && envs.iter().all(|(_, v)| v.is_none());

    // Build the new Tokio command. The wrapper program runs the prefix
    // args (e.g. -f profile.sb --) followed by the original program
    // and its args.
    let mut new = TokioCommand::new(program);
    new.args(prefix_args);
    new.arg(original_program);
    new.args(original_args);

    if let Some(c) = cwd {
        new.current_dir(c);
    }
    if env_clear {
        new.env_clear();
    } else {
        for (k, v) in envs {
            if let Some(val) = v {
                new.env(k, val);
            } else {
                new.env_remove(k);
            }
        }
    }

    // Stdio: we can't reliably read back the existing stdio config, so
    // we leave it to the caller. BashTool sets stdio *before* calling
    // wrap_command — to preserve those settings we re-apply the same
    // sensible defaults the BashTool uses (stdin null, piped out/err).
    // Callers who want different stdio must call wrap_command first
    // and configure stdio on the resulting Command afterward.
    new.stdin(Stdio::null());
    new.stdout(Stdio::piped());
    new.stderr(Stdio::piped());
    new.kill_on_drop(true);
    #[cfg(unix)]
    new.process_group(0);

    *cmd = new;
}

/// Sanity-check internal-consistency invariants of `policy`.
fn validate_policy(policy: &Policy) -> Result<(), SandboxError> {
    if policy.network.http_proxy_port != 0 && policy.network.socks_proxy_port != 0 {
        return Err(SandboxError::InvalidConfig {
            reason: "set at most one of network.http_proxy_port or network.socks_proxy_port".into(),
        });
    }
    // Per-hostname allow/deny lists can only be enforced by the loopback proxy —
    // neither bwrap nor Seatbelt can filter egress by hostname. Without a proxy
    // port, honoring `allowed_domains` previously meant keeping the network
    // namespace open (opening ALL egress — an inversion of intent), while
    // `denied_domains` was silently ignored entirely. Both are worse than
    // refusing, so fail closed on the misconfiguration (#403).
    let proxy_set = policy.network.http_proxy_port != 0 || policy.network.socks_proxy_port != 0;
    if !proxy_set
        && (!policy.network.allowed_domains.is_empty() || !policy.network.denied_domains.is_empty())
    {
        return Err(SandboxError::InvalidConfig {
            reason: "network.allowed_domains/denied_domains require network.http_proxy_port or \
                     network.socks_proxy_port to enforce per-hostname rules; set a proxy port \
                     (the proxy applies the domain rules) or remove the domain lists"
                .into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FilesystemAcl, NetworkAcl};

    #[tokio::test]
    async fn disabled_shim_is_noop() {
        let policy = Policy::default();
        let shim = SandboxedShim::new(policy).expect("new ok");
        let mut cmd = TokioCommand::new("/bin/echo");
        cmd.arg("hi");
        shim.wrap_command(&mut cmd, "echo hi").expect("wrap ok");

        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "/bin/echo");
        let args: Vec<_> = std_cmd.get_args().collect();
        assert_eq!(args, vec!["hi"]);
        assert!(!shim.is_active());
    }

    #[tokio::test]
    async fn unsandboxed_command_bypasses_wrap() {
        let policy = Policy {
            enabled: true,
            allow_unsandboxed_commands: vec!["git".into()],
            ..Policy::default()
        };

        // Force a specific backend so we can observe wrap behavior even
        // on CI without the binary. We construct the shim with the
        // backend pre-set instead of calling SandboxedShim::new.
        let shim = SandboxedShim {
            backend: Backend::Bwrap {
                path: "/usr/bin/bwrap".into(),
                version: "0.7".into(),
            },
            policy,
            _seatbelt_profile: None,
            seatbelt_profile_path: None,
        };

        let mut cmd = TokioCommand::new("/bin/sh");
        cmd.arg("-c").arg("git status");
        shim.wrap_command(&mut cmd, "git status").expect("wrap ok");

        // No wrap happened — program is still /bin/sh.
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "/bin/sh");
    }

    #[tokio::test]
    async fn wrap_with_bwrap_replaces_program_and_prepends_args() {
        let policy = Policy {
            enabled: true,
            filesystem: FilesystemAcl {
                allow_write: vec![PathBuf::from("/tmp")],
                ..FilesystemAcl::default()
            },
            network: NetworkAcl::default(),
            ..Policy::default()
        };
        let shim = SandboxedShim {
            backend: Backend::Bwrap {
                path: "/usr/bin/bwrap".into(),
                version: "0.7".into(),
            },
            policy,
            _seatbelt_profile: None,
            seatbelt_profile_path: None,
        };

        let mut cmd = TokioCommand::new("/bin/sh");
        cmd.arg("-c").arg("echo hi");
        shim.wrap_command(&mut cmd, "echo hi").expect("wrap ok");

        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "/usr/bin/bwrap");
        let args: Vec<_> = std_cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--die-with-parent".to_string()));
        assert!(args.iter().any(|a| a == "--bind-try"));
        // The original program follows `--`.
        let dd = args.iter().position(|a| a == "--").expect("-- separator");
        assert_eq!(args[dd + 1], "/bin/sh");
        assert_eq!(args[dd + 2], "-c");
        assert_eq!(args[dd + 3], "echo hi");
    }

    #[tokio::test]
    async fn wrap_with_seatbelt_uses_profile_file() {
        let policy = Policy {
            enabled: true,
            ..Policy::default()
        };
        // Render and write a profile by hand so we don't depend on
        // `detect()` finding sandbox-exec on the host.
        let text = seatbelt::render_profile(&policy);
        let tf = write_profile(&text).expect("write profile");
        let path = tf.path().to_path_buf();

        let shim = SandboxedShim {
            backend: Backend::Seatbelt {
                path: "/usr/bin/sandbox-exec".into(),
            },
            policy,
            seatbelt_profile_path: Some(path.clone()),
            _seatbelt_profile: Some(tf),
        };

        let mut cmd = TokioCommand::new("/bin/echo");
        cmd.arg("hi");
        shim.wrap_command(&mut cmd, "echo hi").expect("wrap ok");

        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "/usr/bin/sandbox-exec");
        let args: Vec<_> = std_cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-f");
        assert_eq!(args[1], path.to_string_lossy());
        assert_eq!(args[2], "--");
        assert_eq!(args[3], "/bin/echo");
        assert_eq!(args[4], "hi");
    }

    #[tokio::test]
    async fn auto_allows_bash_only_when_flag_and_active() {
        // Disabled: false even if flag set.
        let p = Policy {
            enabled: false,
            auto_allow_bash_if_sandboxed: true,
            ..Policy::default()
        };
        let s = SandboxedShim::new(p).expect("new");
        assert!(!s.auto_allows_bash());

        // Enabled + Backend::Unavailable: not active → not auto-allowed.
        let p = Policy {
            enabled: true,
            auto_allow_bash_if_sandboxed: true,
            ..Policy::default()
        };
        let s = SandboxedShim {
            backend: Backend::Unavailable,
            policy: p,
            _seatbelt_profile: None,
            seatbelt_profile_path: None,
        };
        assert!(!s.auto_allows_bash());

        // Active + flag = yes.
        let p = Policy {
            enabled: true,
            auto_allow_bash_if_sandboxed: true,
            ..Policy::default()
        };
        let s = SandboxedShim {
            backend: Backend::Bwrap {
                path: "/usr/bin/bwrap".into(),
                version: "0.7".into(),
            },
            policy: p,
            _seatbelt_profile: None,
            seatbelt_profile_path: None,
        };
        assert!(s.is_active());
        assert!(s.auto_allows_bash());

        // Active + flag off: not auto-allowed.
        let p = Policy {
            enabled: true,
            auto_allow_bash_if_sandboxed: false,
            ..Policy::default()
        };
        let s = SandboxedShim {
            backend: Backend::Bwrap {
                path: "/usr/bin/bwrap".into(),
                version: "0.7".into(),
            },
            policy: p,
            _seatbelt_profile: None,
            seatbelt_profile_path: None,
        };
        assert!(!s.auto_allows_bash());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn seatbelt_profile_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let text = "(version 1)\n(deny default)\n";
        let tf = write_profile(text).expect("write");
        let md = std::fs::metadata(tf.path()).expect("metadata");
        let mode = md.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    #[test]
    fn domain_lists_without_proxy_are_rejected() {
        // #403: allowed_domains / denied_domains are unenforceable without a
        // proxy, so the shim must fail closed rather than silently no-op
        // (denied_domains) or open all egress (allowed_domains).
        for net in [
            NetworkAcl {
                allowed_domains: vec!["github.com".into()],
                ..NetworkAcl::default()
            },
            NetworkAcl {
                denied_domains: vec!["evil.com".into()],
                ..NetworkAcl::default()
            },
        ] {
            let p = Policy {
                enabled: true,
                network: net,
                ..Policy::default()
            };
            let err =
                SandboxedShim::new(p).expect_err("domain list without proxy must be rejected");
            assert!(
                matches!(err, SandboxError::InvalidConfig { .. }),
                "got {err:?}"
            );
        }
    }

    #[test]
    fn domain_lists_with_proxy_are_accepted() {
        // With a proxy port set, the proxy enforces the domain rules → valid.
        let p = Policy {
            enabled: true,
            network: NetworkAcl {
                allowed_domains: vec!["github.com".into()],
                denied_domains: vec!["evil.com".into()],
                http_proxy_port: 8888,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        // Construction validates the policy; backend resolution may still yield
        // Unavailable on a host without bwrap, but it must not be InvalidConfig.
        let result = SandboxedShim::new(p);
        assert!(
            !matches!(result, Err(SandboxError::InvalidConfig { .. })),
            "domain lists with a proxy must not be rejected as InvalidConfig"
        );
    }

    #[test]
    fn invalid_config_both_proxy_ports_rejected() {
        let p = Policy {
            enabled: true,
            network: NetworkAcl {
                http_proxy_port: 8888,
                socks_proxy_port: 1080,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let err = SandboxedShim::new(p).expect_err("should reject");
        assert!(matches!(err, SandboxError::InvalidConfig { .. }));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shim_is_noop_warning() {
        // enabled+default fail_if_unavailable=false → no-op + warning.
        let p = Policy {
            enabled: true,
            ..Policy::default()
        };
        let s = SandboxedShim::new(p).expect("noop on windows");
        assert!(!s.is_active());
    }
}
