//! `caliband` — caliban's per-workspace supervisor daemon binary (ADR 0037).
//!
//! Usage (rarely invoked directly — the `caliban` CLI auto-spawns this
//! binary on first need):
//!
//! ```text
//! caliband --workspace-root /path/to/workspace  # (or --repo-root, alias)
//!         [--socket-path /custom/path.sock]
//!         [--data-base /custom/data/dir]
//!         [--listen 0.0.0.0:7070]          # network (TCP) server mode (#280)
//!         [--advertise-host caliband.pod]  # host clients dial for agents
//!         [--agent-port-base 7100]
//!         [--tls-cert cert.pem --tls-key key.pem [--tls-ca ca.pem]]
//!         [--tls-server-name caliband]     # SAN workers verify (else inherited/unset, #512)
//!         [--token <bearer>]
//! ```
//!
//! When `--listen` (or `CALIBAN_DAEMON_LISTEN`) is absent, the daemon runs in
//! the historical Unix-socket mode, unchanged.

#![allow(clippy::missing_errors_doc)]

use std::path::PathBuf;
use std::sync::Arc;

use caliban_supervisor::store::AgentStore;
use caliban_supervisor::transport::{BindSpec, Endpoint, tls_server_from_pem};
use caliban_supervisor::{NetworkConfig, Supervisor, workspace_socket_path};

#[derive(Debug, Default)]
struct Args {
    workspace_root: Option<PathBuf>,
    socket_path: Option<PathBuf>,
    data_base: Option<PathBuf>,
    // Network (TCP) server mode (#280 Task 7).
    listen: Option<String>,
    advertise_host: Option<String>,
    agent_port_base: Option<u16>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    tls_ca: Option<PathBuf>,
    /// Server name a worker validates the control listener's cert against when
    /// reporting status over TLS (#510). Must match a SAN on the serving cert
    /// (e.g. the daemon's k8s Service name). #512: never defaulted to the
    /// advertise host (the dial name ≠ what the cert proves) — when unset, the
    /// worker keeps any inherited `CALIBAN_CONTROL_TLS_SERVER_NAME` instead.
    tls_server_name: Option<String>,
    token: Option<String>,
}

/// Read an env var, returning `None` for absent/empty.
fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn parse_args() -> Result<Args, String> {
    parse_from(std::env::args().skip(1))
}

/// Parse caliband args from an explicit token iterator. Split out from
/// [`parse_args`] so the `#[cfg(test)]` drift guard can feed
/// `caliban_contract::launch::CalibandLaunch::args()` through the real parser and
/// assert every emitted flag is recognized (#656).
fn parse_from(tokens: impl Iterator<Item = String>) -> Result<Args, String> {
    // Flag names come from the contract crate's constants (#656) — the same
    // source of truth `CalibandLaunch::args()` emits, so the two cannot drift
    // (see the `caliband_launch_args_are_recognized_by_the_parser` drift test).
    use caliban_contract::launch as l;
    let mut a = Args::default();
    let mut it = tokens;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            a_flag if a_flag == l::FLAG_WORKSPACE_ROOT || a_flag == l::FLAG_REPO_ROOT => {
                a.workspace_root = it.next().map(PathBuf::from);
            }
            a_flag if a_flag == l::FLAG_SOCKET_PATH => a.socket_path = it.next().map(PathBuf::from),
            a_flag if a_flag == l::FLAG_DATA_BASE => a.data_base = it.next().map(PathBuf::from),
            a_flag if a_flag == l::FLAG_LISTEN => a.listen = it.next(),
            a_flag if a_flag == l::FLAG_ADVERTISE_HOST => a.advertise_host = it.next(),
            a_flag if a_flag == l::FLAG_AGENT_PORT_BASE => {
                a.agent_port_base = Some(
                    it.next()
                        .ok_or_else(|| format!("{} needs a value", l::FLAG_AGENT_PORT_BASE))?
                        .parse()
                        .map_err(|e| format!("{}: {e}", l::FLAG_AGENT_PORT_BASE))?,
                );
            }
            a_flag if a_flag == l::FLAG_TLS_CERT => a.tls_cert = it.next().map(PathBuf::from),
            a_flag if a_flag == l::FLAG_TLS_KEY => a.tls_key = it.next().map(PathBuf::from),
            a_flag if a_flag == l::FLAG_TLS_CA => a.tls_ca = it.next().map(PathBuf::from),
            a_flag if a_flag == l::FLAG_TLS_SERVER_NAME => a.tls_server_name = it.next(),
            a_flag if a_flag == l::FLAG_TOKEN => a.token = it.next(),
            "-h" | "--help" => {
                eprintln!(
                    "Usage: caliband --workspace-root <path> [--repo-root <path>] [--socket-path <path>]\n\
                     \x20               [--data-base <path>] [--listen <host:port>]\n\
                     \x20               [--advertise-host <host>] [--agent-port-base <port>]\n\
                     \x20               [--tls-cert <pem> --tls-key <pem>] [--tls-ca <pem>]\n\
                     \x20               [--tls-server-name <name>] [--token <bearer>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    // Env fallbacks (flags win). Env names come from the contract constants too.
    a.listen = a.listen.or_else(|| env_opt(l::ENV_DAEMON_LISTEN));
    a.advertise_host = a
        .advertise_host
        .or_else(|| env_opt(l::ENV_DAEMON_ADVERTISE_HOST));
    if a.agent_port_base.is_none()
        && let Some(v) = env_opt(l::ENV_DAEMON_AGENT_PORT_BASE)
    {
        a.agent_port_base = Some(
            v.parse()
                .map_err(|e| format!("{}: {e}", l::ENV_DAEMON_AGENT_PORT_BASE))?,
        );
    }
    a.tls_cert = a
        .tls_cert
        .or_else(|| env_opt(l::ENV_DAEMON_TLS_CERT).map(PathBuf::from));
    a.tls_key = a
        .tls_key
        .or_else(|| env_opt(l::ENV_DAEMON_TLS_KEY).map(PathBuf::from));
    a.tls_ca = a
        .tls_ca
        .or_else(|| env_opt(l::ENV_DAEMON_TLS_CA).map(PathBuf::from));
    a.tls_server_name = a
        .tls_server_name
        .or_else(|| env_opt(l::ENV_DAEMON_TLS_SERVER_NAME));
    a.token = a.token.or_else(|| env_opt(l::ENV_DAEMON_TOKEN));

    if a.workspace_root.is_none() {
        return Err("--workspace-root required (or --repo-root)".to_string());
    }
    Ok(a)
}

/// The host part of a `host:port` string (everything before the last `:`).
fn host_of(addr: &str) -> &str {
    addr.rsplit_once(':').map_or(addr, |(host, _)| host)
}

/// Whether `host` is a wildcard / unspecified bind address that clients cannot
/// dial (#319). `--listen 0.0.0.0:P` with no `--advertise-host` derives such a
/// host (via [`host_of`]), which then names undialable agent endpoints and a
/// `0.0.0.0` control endpoint reported in `daemon status`. Detects the IPv4
/// (`0.0.0.0`) and IPv6 (`::`, `[::]`) unspecified addresses plus the empty
/// string and `*`.
fn is_wildcard_host(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }
    // Strip optional IPv6 literal brackets (`[::]` → `::`).
    let bare = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    bare.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_unspecified())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Minimal tracing setup so log lines reach stderr.
    tracing_subscriber_init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("caliband: {e}");
            std::process::exit(2);
        }
    };

    // `parse_args` guarantees `workspace_root` is set.
    let workspace_root = args
        .workspace_root
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let socket_path = args
        .socket_path
        .clone()
        .unwrap_or_else(|| workspace_socket_path(&workspace_root));
    let agent_runtime_dir = socket_path.parent().map_or_else(
        || std::env::temp_dir().join("caliban-agents"),
        |p| p.join("agents"),
    );
    let store = if let Some(base) = args.data_base.clone() {
        AgentStore::new(base)
    } else {
        AgentStore::default_for(&workspace_root)
    };

    let supervisor = match build_supervisor(
        &args,
        socket_path,
        store,
        agent_runtime_dir,
        workspace_root.clone(),
    ) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("caliband: {e}");
            std::process::exit(2);
        }
    };

    // SIGTERM handling: cancel the supervisor on receipt so the bind
    // socket gets cleaned up before we exit.
    #[cfg(unix)]
    {
        let token = supervisor.cancel_token();
        tokio::spawn(async move {
            if let Ok(mut sig) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                sig.recv().await;
                tracing::info!("caliband: SIGTERM received");
                token.cancel();
            }
        });
    }

    supervisor.serve().await
}

/// Build the supervisor for the requested mode. `--listen` (or
/// `CALIBAN_DAEMON_LISTEN`) selects TCP network mode (#280 Task 7); otherwise
/// the historical Unix-socket mode.
fn build_supervisor(
    args: &Args,
    socket_path: PathBuf,
    store: AgentStore,
    agent_runtime_dir: PathBuf,
    workspace_root: PathBuf,
) -> Result<Supervisor, String> {
    let Some(listen) = args.listen.clone() else {
        // Unix mode (default, unchanged).
        return Ok(Supervisor::new(socket_path, store, agent_runtime_dir)
            .with_workspace_root(workspace_root));
    };

    // --- Network (TCP) mode. ---
    // Control-listener TLS: load only when both cert and key are given.
    let control_tls = match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => {
            let cert_pem = std::fs::read(cert).map_err(|e| format!("--tls-cert: {e}"))?;
            let key_pem = std::fs::read(key).map_err(|e| format!("--tls-key: {e}"))?;
            Some(tls_server_from_pem(&cert_pem, &key_pem).map_err(|e| format!("TLS: {e}"))?)
        }
        (None, None) => None,
        _ => return Err("--tls-cert and --tls-key must be given together".to_string()),
    };
    // Per-agent listeners reuse the same TLS material as the control plane, so
    // the worker binds a symmetric secure socket. Loaded once to fail fast.
    let agent_tls = match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => {
            let cert_pem = std::fs::read(cert).map_err(|e| format!("--tls-cert: {e}"))?;
            let key_pem = std::fs::read(key).map_err(|e| format!("--tls-key: {e}"))?;
            Some(tls_server_from_pem(&cert_pem, &key_pem).map_err(|e| format!("TLS: {e}"))?)
        }
        _ => None,
    };

    let advertise_host = args
        .advertise_host
        .clone()
        .unwrap_or_else(|| host_of(&listen).to_string());
    // #319: a wildcard advertise host (e.g. derived from `--listen 0.0.0.0:P`
    // with no `--advertise-host`) yields agent endpoints and a control endpoint
    // no client can dial. Flag it at startup so it is observable before a
    // stranded agent, not after.
    if is_wildcard_host(&advertise_host) {
        tracing::warn!(
            advertise_host = %advertise_host,
            "advertise host is a wildcard/unspecified address; agent endpoints and the \
             control endpoint reported in `daemon status` will be undialable by clients. \
             Set --advertise-host to a routable name (e.g. the daemon's Service/pod DNS name)."
        );
    }
    let agent_port_base = args.agent_port_base.unwrap_or(7100);

    // Fail-closed: never bind the control plane unauthenticated or in plaintext.
    // The #288 fix guarded only the per-agent worker listener, leaving the daemon
    // control socket fail-open (#400) — apply the same policy here.
    caliban_supervisor::require_network_credentials(args.token.as_deref(), control_tls.is_some())
        .map_err(|e| format!("--listen: {e}"))?;

    let bind = BindSpec {
        endpoint: Endpoint::Tcp { addr: listen },
        tls: control_tls,
        token: args.token.clone(),
    };
    let network = NetworkConfig {
        advertise_host: advertise_host.clone(),
        agent_port_base,
        agent_tls,
        agent_token: args.token.clone(),
    };

    // Wire the worker launcher: it execs `caliban __agent-worker --listen ...`
    // and passes per-agent TLS/token + the daemon control endpoint via env so
    // the worker can secure its own listener and report status back.
    let control_endpoint = network_control_endpoint(&advertise_host, args);
    // #510: forward the control-plane CA so the worker can dial the (TLS)
    // control listener to report status — only when caliband itself was given
    // one (--tls-ca); it cannot forward a CA it never received.
    //
    // #512: the server name is resolved separately and is NEVER derived from the
    // advertise host. Forward only an explicit --tls-server-name /
    // CALIBAN_DAEMON_TLS_SERVER_NAME; otherwise leave the worker var unset so an
    // operator's inherited CALIBAN_CONTROL_TLS_SERVER_NAME stands (#510 silently
    // clobbered it with the advertise host, breaking a config that had worked by
    // ordinary env inheritance). If control TLS is on but no name is resolvable
    // anywhere, warn at startup — the worker would verify against `localhost`,
    // which the serving cert cannot prove, and every status report would fail
    // silently.
    let (control_tls_server_name, warn_missing_server_name) = resolve_control_tls_server_name(
        args.tls_server_name.as_deref(),
        args.tls_ca.is_some(),
        env_opt("CALIBAN_CONTROL_TLS_SERVER_NAME").as_deref(),
    );
    if warn_missing_server_name {
        tracing::warn!(
            "control-plane TLS is configured (--tls-ca) but no worker TLS server name is set \
             (neither --tls-server-name / CALIBAN_DAEMON_TLS_SERVER_NAME nor an inherited \
             CALIBAN_CONTROL_TLS_SERVER_NAME); workers will verify the control listener against \
             `localhost`, which the serving cert almost certainly cannot prove, so status \
             reports will fail. Set --tls-server-name to the cert's SAN (e.g. the daemon's \
             Service name)."
        );
    }
    let launcher = Arc::new(
        caliban_supervisor::ExecWorkerLauncher::sibling_of_current_exe()
            .with_agent_network(
                args.tls_cert.clone(),
                args.tls_key.clone(),
                args.token.clone(),
                control_endpoint,
            )
            .with_control_tls(args.tls_ca.clone(), control_tls_server_name),
    );

    Ok(
        Supervisor::with_bind(bind, Some(network), store, agent_runtime_dir, launcher)
            .with_workspace_root(workspace_root),
    )
}

/// Derive the control endpoint (`host:port`) a worker dials to report status
/// over the network: the advertise host + the control listener's port. QA
/// note: single-pod assumption; a multi-host deployment may need an explicit
/// override. Returns `None` if the listen port can't be determined.
fn network_control_endpoint(advertise_host: &str, args: &Args) -> Option<String> {
    let listen = args.listen.as_deref()?;
    let port = listen.rsplit_once(':').map(|(_, p)| p)?;
    Some(format!("{advertise_host}:{port}"))
}

/// Resolve the TLS server name to forward to workers for verifying the daemon's
/// control listener, plus whether the resulting config is a silent-failure risk
/// worth warning about at startup (#512).
///
/// The server name is **never** derived from the advertise host: the advertise
/// host is *where workers dial*, the server name is *what the cert must prove*,
/// and they coincide only when the serving cert carries the dial name as a SAN —
/// false for the k8s Service topology this feature exists for. #510 defaulted to
/// the advertise host and stranded a live cluster.
///
/// - `configured`: explicit `--tls-server-name` / `CALIBAN_DAEMON_TLS_SERVER_NAME`.
/// - `tls_ca_set`: whether control TLS is in play (`--tls-ca`); without it
///   workers dial plaintext by design and a server name is irrelevant.
/// - `inherited`: `CALIBAN_CONTROL_TLS_SERVER_NAME` in the daemon's own env,
///   which the worker inherits when the launcher forwards nothing.
///
/// Returns `(name_to_forward, warn)`. `name_to_forward` is `None` unless a name
/// was explicitly configured, so an inherited/explicit worker value stands
/// rather than being clobbered by a derived one. `warn` is `true` only when
/// control TLS is on yet no name is resolvable from any source — the worker
/// would then fall back to `localhost`, which the serving cert almost certainly
/// cannot prove, and every status report would fail.
fn resolve_control_tls_server_name(
    configured: Option<&str>,
    tls_ca_set: bool,
    inherited: Option<&str>,
) -> (Option<String>, bool) {
    let forwarded = configured.map(str::to_string);
    let warn = tls_ca_set && forwarded.is_none() && inherited.is_none();
    (forwarded, warn)
}

fn tracing_subscriber_init() {
    // No-op: callers can set RUST_LOG to enable; we skip a heavy
    // subscriber setup for the binary entry point so the daemon stays
    // light. (`caliban` itself wires the file-based subscriber.)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #656 drift guard: caliband's parser and `CalibandLaunch::args()` share the
    /// contract's flag-name constants, so a fully-populated launch spec must parse
    /// cleanly through the REAL parser and round-trip every value. A flag renamed
    /// on one side but not the other — the class of bug that caused
    /// caliban-operator#30/#32/#35/#44 — fails here.
    #[test]
    fn caliband_launch_args_are_recognized_by_the_parser() {
        use caliban_contract::launch::{CalibandLaunch, CalibandTls};
        use std::path::Path;

        let launch = CalibandLaunch {
            socket_path: Some("/x.sock".into()),
            data_base: Some("/data".into()),
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
            router_config: Some("{}".into()), // env-only; not emitted by args()
            ..CalibandLaunch::new("/repo")
        };
        let parsed = parse_from(launch.args().into_iter())
            .expect("caliband must recognize every flag CalibandLaunch::args() emits");
        assert_eq!(parsed.workspace_root.as_deref(), Some(Path::new("/repo")));
        assert_eq!(parsed.socket_path.as_deref(), Some(Path::new("/x.sock")));
        assert_eq!(parsed.data_base.as_deref(), Some(Path::new("/data")));
        assert_eq!(parsed.listen.as_deref(), Some("0.0.0.0:7070"));
        assert_eq!(parsed.advertise_host.as_deref(), Some("caliband.pod"));
        assert_eq!(parsed.agent_port_base, Some(7100));
        assert_eq!(parsed.tls_cert.as_deref(), Some(Path::new("/tls/cert.pem")));
        assert_eq!(parsed.tls_key.as_deref(), Some(Path::new("/tls/key.pem")));
        assert_eq!(parsed.tls_ca.as_deref(), Some(Path::new("/tls/ca.pem")));
        assert_eq!(parsed.tls_server_name.as_deref(), Some("caliband"));
        assert_eq!(parsed.token.as_deref(), Some("s3cret"));
    }

    /// #512: the worker's TLS verification identity must never be *derived* from
    /// the advertise host. The advertise host is where workers dial; the server
    /// name is what the cert must prove. #510 defaulted the name to the
    /// advertise host and stranded a live cluster (cert SAN was `caliband`, the
    /// dial name a k8s Service FQDN). With no name configured anywhere, forward
    /// nothing.
    #[test]
    fn server_name_is_not_derived_from_the_advertise_host() {
        let (forwarded, _warn) = resolve_control_tls_server_name(None, true, None);
        assert_eq!(
            forwarded, None,
            "an unconfigured server name must not be filled in with a derived value"
        );
    }

    /// An explicitly-configured name (`--tls-server-name` /
    /// `CALIBAN_DAEMON_TLS_SERVER_NAME`) is forwarded verbatim and needs no
    /// warning — the operator said what the cert proves.
    #[test]
    fn explicit_server_name_is_forwarded_without_warning() {
        let (forwarded, warn) = resolve_control_tls_server_name(Some("caliband"), true, None);
        assert_eq!(forwarded, Some("caliband".to_string()));
        assert!(
            !warn,
            "an explicit server name is not a silent-failure risk"
        );
    }

    /// #512 fix #2: an operator's inherited `CALIBAN_CONTROL_TLS_SERVER_NAME`
    /// (present in the daemon's env, which the worker inherits) must stand — so
    /// caliband forwards nothing (no clobber) and, because a value exists, does
    /// not warn.
    #[test]
    fn inherited_env_value_is_left_to_stand_without_warning() {
        let (forwarded, warn) = resolve_control_tls_server_name(None, true, Some("caliband"));
        assert_eq!(
            forwarded, None,
            "must not forward a derived value that would clobber the inherited one"
        );
        assert!(!warn, "an inherited value is not a silent-failure risk");
    }

    /// #512 AC#1: control TLS in play (`--tls-ca` set) with no server name
    /// resolvable from any source is the silent-failure config — the worker
    /// would verify against `localhost`, which the serving cert cannot prove.
    /// caliband must flag it so it is observable at startup, not only after a
    /// stranded agent.
    #[test]
    fn control_tls_without_any_server_name_warns() {
        let (forwarded, warn) = resolve_control_tls_server_name(None, true, None);
        assert_eq!(forwarded, None);
        assert!(
            warn,
            "a TLS control listener with no resolvable server name must be flagged"
        );
    }

    /// No control TLS (`--tls-ca` unset) means workers dial plaintext by design;
    /// a missing server name is irrelevant, so there is nothing to warn about.
    #[test]
    fn no_control_tls_never_warns() {
        let (forwarded, warn) = resolve_control_tls_server_name(None, false, None);
        assert_eq!(forwarded, None);
        assert!(
            !warn,
            "without control TLS there is no verification to misconfigure"
        );
    }

    /// #319: a wildcard/unspecified advertise host (the value `--listen
    /// 0.0.0.0:P` derives with no `--advertise-host`) is undialable and must be
    /// flagged; a real hostname or routable IP must not be.
    #[test]
    fn is_wildcard_host_flags_only_unspecified_addresses() {
        for wildcard in ["0.0.0.0", "::", "[::]", "*", "", "  0.0.0.0  "] {
            assert!(is_wildcard_host(wildcard), "should flag {wildcard:?}");
        }
        for routable in [
            "caliband",
            "caliband.caliban.svc.cluster.local",
            "10.0.0.5",
            "127.0.0.1",
            "::1",
        ] {
            assert!(!is_wildcard_host(routable), "should not flag {routable:?}");
        }
    }
}
