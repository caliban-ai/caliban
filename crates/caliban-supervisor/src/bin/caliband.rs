//! `caliband` — caliban's per-repo supervisor daemon binary (ADR 0037).
//!
//! Usage (rarely invoked directly — the `caliban` CLI auto-spawns this
//! binary on first need):
//!
//! ```text
//! caliband --repo-root /path/to/repo
//!         [--socket-path /custom/path.sock]
//!         [--data-base /custom/data/dir]
//! ```

#![allow(clippy::missing_errors_doc)]

use std::path::PathBuf;
use std::sync::Arc;

use caliban_supervisor::store::AgentStore;
use caliban_supervisor::{Supervisor, repo_socket_path};

#[derive(Debug)]
struct Args {
    repo_root: PathBuf,
    socket_path: Option<PathBuf>,
    data_base: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut repo_root: Option<PathBuf> = None;
    let mut socket_path: Option<PathBuf> = None;
    let mut data_base: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--repo-root" => repo_root = it.next().map(PathBuf::from),
            "--socket-path" => socket_path = it.next().map(PathBuf::from),
            "--data-base" => data_base = it.next().map(PathBuf::from),
            "-h" | "--help" => {
                eprintln!(
                    "Usage: caliband --repo-root <path> [--socket-path <path>] [--data-base <path>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    let repo_root = repo_root.ok_or_else(|| "--repo-root required".to_string())?;
    Ok(Args {
        repo_root,
        socket_path,
        data_base,
    })
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

    let socket_path = args
        .socket_path
        .clone()
        .unwrap_or_else(|| repo_socket_path(&args.repo_root));
    let agent_runtime_dir = socket_path.parent().map_or_else(
        || std::env::temp_dir().join("caliban-agents"),
        |p| p.join("agents"),
    );
    let store = if let Some(base) = args.data_base.clone() {
        AgentStore::new(base)
    } else {
        AgentStore::default_for(&args.repo_root)
    };

    let supervisor = Arc::new(Supervisor::new(socket_path, store, agent_runtime_dir));

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

fn tracing_subscriber_init() {
    // No-op: callers can set RUST_LOG to enable; we skip a heavy
    // subscriber setup for the binary entry point so the daemon stays
    // light. (`caliban` itself wires the file-based subscriber.)
}
