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
    token: Option<String>,
}

/// Read an env var, returning `None` for absent/empty.
fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--workspace-root" | "--repo-root" => {
                a.workspace_root = it.next().map(PathBuf::from);
            }
            "--socket-path" => a.socket_path = it.next().map(PathBuf::from),
            "--data-base" => a.data_base = it.next().map(PathBuf::from),
            "--listen" => a.listen = it.next(),
            "--advertise-host" => a.advertise_host = it.next(),
            "--agent-port-base" => {
                a.agent_port_base = Some(
                    it.next()
                        .ok_or_else(|| "--agent-port-base needs a value".to_string())?
                        .parse()
                        .map_err(|e| format!("--agent-port-base: {e}"))?,
                );
            }
            "--tls-cert" => a.tls_cert = it.next().map(PathBuf::from),
            "--tls-key" => a.tls_key = it.next().map(PathBuf::from),
            "--tls-ca" => a.tls_ca = it.next().map(PathBuf::from),
            "--token" => a.token = it.next(),
            "-h" | "--help" => {
                eprintln!(
                    "Usage: caliband --workspace-root <path> [--repo-root <path>] [--socket-path <path>]\n\
                     \x20               [--data-base <path>] [--listen <host:port>]\n\
                     \x20               [--advertise-host <host>] [--agent-port-base <port>]\n\
                     \x20               [--tls-cert <pem> --tls-key <pem>] [--tls-ca <pem>]\n\
                     \x20               [--token <bearer>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    // Env fallbacks (flags win).
    a.listen = a.listen.or_else(|| env_opt("CALIBAN_DAEMON_LISTEN"));
    a.advertise_host = a
        .advertise_host
        .or_else(|| env_opt("CALIBAN_DAEMON_ADVERTISE_HOST"));
    if a.agent_port_base.is_none()
        && let Some(v) = env_opt("CALIBAN_DAEMON_AGENT_PORT_BASE")
    {
        a.agent_port_base = Some(
            v.parse()
                .map_err(|e| format!("CALIBAN_DAEMON_AGENT_PORT_BASE: {e}"))?,
        );
    }
    a.tls_cert = a
        .tls_cert
        .or_else(|| env_opt("CALIBAN_DAEMON_TLS_CERT").map(PathBuf::from));
    a.tls_key = a
        .tls_key
        .or_else(|| env_opt("CALIBAN_DAEMON_TLS_KEY").map(PathBuf::from));
    a.tls_ca = a
        .tls_ca
        .or_else(|| env_opt("CALIBAN_DAEMON_TLS_CA").map(PathBuf::from));
    a.token = a.token.or_else(|| env_opt("CALIBAN_DAEMON_TOKEN"));

    if a.workspace_root.is_none() {
        return Err("--workspace-root required (or --repo-root)".to_string());
    }
    Ok(a)
}

/// The host part of a `host:port` string (everything before the last `:`).
fn host_of(addr: &str) -> &str {
    addr.rsplit_once(':').map_or(addr, |(host, _)| host)
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
    let agent_port_base = args.agent_port_base.unwrap_or(7100);

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
    let launcher = Arc::new(
        caliban_supervisor::ExecWorkerLauncher::sibling_of_current_exe().with_agent_network(
            args.tls_cert.clone(),
            args.tls_key.clone(),
            args.token.clone(),
            control_endpoint,
        ),
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

fn tracing_subscriber_init() {
    // No-op: callers can set RUST_LOG to enable; we skip a heavy
    // subscriber setup for the binary entry point so the daemon stays
    // light. (`caliban` itself wires the file-based subscriber.)
}
