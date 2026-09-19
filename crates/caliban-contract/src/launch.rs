//! The caliband launch contract: the flag and environment-variable **names**
//! caliband is launched with, and a typed [`CalibandLaunch`] builder that turns a
//! launch spec into caliband's argv ([`CalibandLaunch::args`]) or environment
//! ([`CalibandLaunch::env`]).
//!
//! caliband's own arg parser references the same `FLAG_*` / `ENV_*` constants, so
//! a flag renamed on one side and not the other fails the drift test in
//! `caliban-supervisor` (#656). caliban-operator builds the k8s container spec
//! from this builder instead of hand-typing the names (the source of
//! caliban-operator#30 / #32 / #35 / #44).

use std::path::{Path, PathBuf};

// --- Flag names (argv form) ------------------------------------------------

/// `--workspace-root <path>` — the repo/workspace the daemon manages (required).
pub const FLAG_WORKSPACE_ROOT: &str = "--workspace-root";
/// `--repo-root <path>` — accepted alias for [`FLAG_WORKSPACE_ROOT`].
pub const FLAG_REPO_ROOT: &str = "--repo-root";
/// `--socket-path <path>` — override the control Unix socket path.
pub const FLAG_SOCKET_PATH: &str = "--socket-path";
/// `--data-base <path>` — override the data base directory.
pub const FLAG_DATA_BASE: &str = "--data-base";
/// `--listen <host:port>` — bind the control plane over TCP (network mode).
pub const FLAG_LISTEN: &str = "--listen";
/// `--advertise-host <host>` — the host clients dial for agents.
pub const FLAG_ADVERTISE_HOST: &str = "--advertise-host";
/// `--agent-port-base <port>` — first port used for per-agent listeners.
pub const FLAG_AGENT_PORT_BASE: &str = "--agent-port-base";
/// `--tls-cert <pem>` — server certificate (network mode).
pub const FLAG_TLS_CERT: &str = "--tls-cert";
/// `--tls-key <pem>` — server private key (network mode).
pub const FLAG_TLS_KEY: &str = "--tls-key";
/// `--tls-ca <pem>` — CA workers verify the control listener against.
pub const FLAG_TLS_CA: &str = "--tls-ca";
/// `--tls-server-name <name>` — SAN workers verify (else inherited/unset, #512).
pub const FLAG_TLS_SERVER_NAME: &str = "--tls-server-name";
/// `--token <bearer>` — control-plane bearer token.
pub const FLAG_TOKEN: &str = "--token";

// --- Environment-variable names (env form) ---------------------------------

/// Env fallback for [`FLAG_LISTEN`].
pub const ENV_DAEMON_LISTEN: &str = "CALIBAN_DAEMON_LISTEN";
/// Env fallback for [`FLAG_ADVERTISE_HOST`].
pub const ENV_DAEMON_ADVERTISE_HOST: &str = "CALIBAN_DAEMON_ADVERTISE_HOST";
/// Env fallback for [`FLAG_AGENT_PORT_BASE`].
pub const ENV_DAEMON_AGENT_PORT_BASE: &str = "CALIBAN_DAEMON_AGENT_PORT_BASE";
/// Env fallback for [`FLAG_TLS_CERT`].
pub const ENV_DAEMON_TLS_CERT: &str = "CALIBAN_DAEMON_TLS_CERT";
/// Env fallback for [`FLAG_TLS_KEY`].
pub const ENV_DAEMON_TLS_KEY: &str = "CALIBAN_DAEMON_TLS_KEY";
/// Env fallback for [`FLAG_TLS_CA`].
pub const ENV_DAEMON_TLS_CA: &str = "CALIBAN_DAEMON_TLS_CA";
/// Env fallback for [`FLAG_TLS_SERVER_NAME`].
pub const ENV_DAEMON_TLS_SERVER_NAME: &str = "CALIBAN_DAEMON_TLS_SERVER_NAME";
/// Env fallback for [`FLAG_TOKEN`].
pub const ENV_DAEMON_TOKEN: &str = "CALIBAN_DAEMON_TOKEN";
/// Model-router config, read by the worker (no flag; env only). caliban-operator#44
/// was a bug from mis-typing this name.
pub const ENV_ROUTER_CONFIG: &str = "CALIBAN_ROUTER_CONFIG";

// --- Provider credential env names -----------------------------------------

/// A provider whose credentials are supplied by environment variables. The
/// base-URL / API-key env names are **not** a uniform `{NAME}_*` — Google reads
/// `GEMINI_*`, not `GOOGLE_*` — so each kind carries its exact names (the mapping
/// caliban-operator#30 got wrong by assuming uniformity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Anthropic (`ANTHROPIC_*`).
    Anthropic,
    /// `OpenAI` (`OPENAI_*`).
    OpenAi,
    /// Google Gemini (`GEMINI_*`).
    Google,
}

impl ProviderKind {
    /// The `SpawnSpec.provider` / `--provider` wire name the worker parses.
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
        }
    }

    /// The env var caliban reads the provider's base-URL override from.
    #[must_use]
    pub fn base_url_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_BASE_URL",
            Self::OpenAi => "OPENAI_BASE_URL",
            Self::Google => "GEMINI_BASE_URL",
        }
    }

    /// The env var caliban reads the provider's API key from.
    #[must_use]
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Google => "GEMINI_API_KEY",
        }
    }
}

// --- Launch builder --------------------------------------------------------

/// TLS material for a network-mode caliband launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibandTls {
    /// Server certificate PEM path (`--tls-cert`).
    pub cert: PathBuf,
    /// Server private key PEM path (`--tls-key`).
    pub key: PathBuf,
    /// Optional CA PEM path workers verify against (`--tls-ca`).
    pub ca: Option<PathBuf>,
    /// Optional SAN workers verify the control listener against
    /// (`--tls-server-name`, #512).
    pub server_name: Option<String>,
}

/// A typed caliband launch spec. [`args`](Self::args) renders the complete
/// flag-based argv (the canonical form the drift test round-trips through
/// caliband's real parser); [`env`](Self::env) renders the environment-variable
/// equivalents for the fields that support them (network config, TLS, token,
/// router config) — the form the k8s operator prefers. `workspace_root` /
/// `socket_path` / `data_base` have no env fallback in caliband, so they appear
/// only in `args()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibandLaunch {
    /// The repo/workspace the daemon manages (required; `--workspace-root`).
    pub workspace_root: PathBuf,
    /// Override the control Unix socket path (`--socket-path`).
    pub socket_path: Option<PathBuf>,
    /// Override the data base directory (`--data-base`).
    pub data_base: Option<PathBuf>,
    /// Bind the control plane over TCP (`--listen` / `CALIBAN_DAEMON_LISTEN`).
    pub listen: Option<String>,
    /// Host clients dial for agents (`--advertise-host`).
    pub advertise_host: Option<String>,
    /// First per-agent listener port (`--agent-port-base`).
    pub agent_port_base: Option<u16>,
    /// TLS material for network mode.
    pub tls: Option<CalibandTls>,
    /// Control-plane bearer token (`--token` / `CALIBAN_DAEMON_TOKEN`).
    pub token: Option<String>,
    /// Model-router config JSON, passed to the worker via env only
    /// (`CALIBAN_ROUTER_CONFIG`).
    pub router_config: Option<String>,
}

impl CalibandLaunch {
    /// A minimal launch spec: just the required workspace root, everything else
    /// unset (local Unix-socket mode).
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            socket_path: None,
            data_base: None,
            listen: None,
            advertise_host: None,
            agent_port_base: None,
            tls: None,
            token: None,
            router_config: None,
        }
    }

    /// Render the complete flag-based argv for launching caliband. Every set
    /// field is emitted using its `FLAG_*` constant, so this is the single source
    /// of truth caliband's parser is drift-tested against.
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let mut out = vec![
            FLAG_WORKSPACE_ROOT.to_string(),
            path_arg(&self.workspace_root),
        ];
        let mut push = |flag: &str, val: String| {
            out.push(flag.to_string());
            out.push(val);
        };
        if let Some(p) = &self.socket_path {
            push(FLAG_SOCKET_PATH, path_arg(p));
        }
        if let Some(p) = &self.data_base {
            push(FLAG_DATA_BASE, path_arg(p));
        }
        if let Some(v) = &self.listen {
            push(FLAG_LISTEN, v.clone());
        }
        if let Some(v) = &self.advertise_host {
            push(FLAG_ADVERTISE_HOST, v.clone());
        }
        if let Some(v) = self.agent_port_base {
            push(FLAG_AGENT_PORT_BASE, v.to_string());
        }
        if let Some(tls) = &self.tls {
            push(FLAG_TLS_CERT, path_arg(&tls.cert));
            push(FLAG_TLS_KEY, path_arg(&tls.key));
            if let Some(ca) = &tls.ca {
                push(FLAG_TLS_CA, path_arg(ca));
            }
            if let Some(name) = &tls.server_name {
                push(FLAG_TLS_SERVER_NAME, name.clone());
            }
        }
        if let Some(v) = &self.token {
            push(FLAG_TOKEN, v.clone());
        }
        out
    }

    /// Render the environment-variable form for the fields that support it
    /// (network config, TLS, token, router config). `workspace_root` /
    /// `socket_path` / `data_base` have no env fallback and are omitted — pass
    /// them via [`args`](Self::args).
    #[must_use]
    pub fn env(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut push = |key: &str, val: String| out.push((key.to_string(), val));
        if let Some(v) = &self.listen {
            push(ENV_DAEMON_LISTEN, v.clone());
        }
        if let Some(v) = &self.advertise_host {
            push(ENV_DAEMON_ADVERTISE_HOST, v.clone());
        }
        if let Some(v) = self.agent_port_base {
            push(ENV_DAEMON_AGENT_PORT_BASE, v.to_string());
        }
        if let Some(tls) = &self.tls {
            push(ENV_DAEMON_TLS_CERT, path_arg(&tls.cert));
            push(ENV_DAEMON_TLS_KEY, path_arg(&tls.key));
            if let Some(ca) = &tls.ca {
                push(ENV_DAEMON_TLS_CA, path_arg(ca));
            }
            if let Some(name) = &tls.server_name {
                push(ENV_DAEMON_TLS_SERVER_NAME, name.clone());
            }
        }
        if let Some(v) = &self.token {
            push(ENV_DAEMON_TOKEN, v.clone());
        }
        if let Some(v) = &self.router_config {
            push(ENV_ROUTER_CONFIG, v.clone());
        }
        out
    }
}

/// Render a path as a launch argument. caliband's paths are UTF-8 in every
/// deployment; a non-UTF-8 path is lossily rendered rather than dropped.
fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_launch_emits_only_workspace_root() {
        let launch = CalibandLaunch::new("/repo");
        assert_eq!(launch.args(), vec![FLAG_WORKSPACE_ROOT, "/repo"]);
        assert!(launch.env().is_empty());
    }

    #[test]
    fn full_network_launch_args_use_the_flag_constants() {
        let launch = CalibandLaunch {
            listen: Some("0.0.0.0:7070".into()),
            advertise_host: Some("caliband.pod".into()),
            agent_port_base: Some(7100),
            tls: Some(CalibandTls {
                cert: "/tls/cert.pem".into(),
                key: "/tls/key.pem".into(),
                ca: Some("/tls/ca.pem".into()),
                server_name: Some("caliband".into()),
            }),
            token: Some("s3cret".into()),
            ..CalibandLaunch::new("/repo")
        };
        let args = launch.args();
        // Spot-check each flag/value pair is present and adjacent.
        for (flag, val) in [
            (FLAG_WORKSPACE_ROOT, "/repo"),
            (FLAG_LISTEN, "0.0.0.0:7070"),
            (FLAG_ADVERTISE_HOST, "caliband.pod"),
            (FLAG_AGENT_PORT_BASE, "7100"),
            (FLAG_TLS_CERT, "/tls/cert.pem"),
            (FLAG_TLS_KEY, "/tls/key.pem"),
            (FLAG_TLS_CA, "/tls/ca.pem"),
            (FLAG_TLS_SERVER_NAME, "caliband"),
            (FLAG_TOKEN, "s3cret"),
        ] {
            let i = args.iter().position(|a| a == flag).expect(flag);
            assert_eq!(args[i + 1], val, "value after {flag}");
        }
    }

    #[test]
    fn env_form_covers_env_settable_fields_only() {
        let launch = CalibandLaunch {
            socket_path: Some("/x.sock".into()), // no env form — must not appear
            listen: Some("0.0.0.0:7070".into()),
            agent_port_base: Some(7100),
            token: Some("s3cret".into()),
            router_config: Some("{}".into()),
            ..CalibandLaunch::new("/repo")
        };
        let env: std::collections::HashMap<_, _> = launch.env().into_iter().collect();
        assert_eq!(env.get(ENV_DAEMON_LISTEN).unwrap(), "0.0.0.0:7070");
        assert_eq!(env.get(ENV_DAEMON_AGENT_PORT_BASE).unwrap(), "7100");
        assert_eq!(env.get(ENV_DAEMON_TOKEN).unwrap(), "s3cret");
        assert_eq!(env.get(ENV_ROUTER_CONFIG).unwrap(), "{}");
        // workspace_root and socket_path have no env fallback in caliband.
        assert!(
            !env.keys()
                .any(|k| k.contains("WORKSPACE") || k.contains("SOCKET"))
        );
    }

    #[test]
    fn provider_env_names_match_what_caliban_reads() {
        // Google reads GEMINI_*, not GOOGLE_* — the non-uniform mapping.
        assert_eq!(ProviderKind::Anthropic.api_key_env(), "ANTHROPIC_API_KEY");
        assert_eq!(ProviderKind::OpenAi.base_url_env(), "OPENAI_BASE_URL");
        assert_eq!(ProviderKind::Google.api_key_env(), "GEMINI_API_KEY");
        assert_eq!(ProviderKind::Google.base_url_env(), "GEMINI_BASE_URL");
        assert_eq!(ProviderKind::Google.wire_name(), "google");
    }
}
