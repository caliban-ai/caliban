//! Top-level subcommand dispatchers for the `caliban` binary.
//!
//! Each function here owns the handling of one branch of the
//! [`crate::CalibanCommand`] tree (or one early-exit shortcut like the
//! `caliban plugin ...` proxy and the `--bg` flag). They are thin
//! wrappers over the per-feature modules — `agents_cli`, `plugin_cli`,
//! `router` — and return either an exit code (for handlers that
//! `std::process::exit` the parent) or a `Result<()>` for handlers
//! that print and return normally.

use anyhow::{Context, Result};

use crate::agents_cli;
use crate::args::{AgentsCommand, CalibanCommand, RouterCommand};
use crate::plugin_cli;
use crate::router;

/// Run the `caliban plugin <subcommand>` proxy and return the exit code
/// the parent should pass to `std::process::exit`. The dispatcher accepts
/// the first positional arg only — `caliban --debug plugin list` is not
/// supported (mirrors how Cargo subcommands work).
pub(crate) async fn run_plugin_cli(forwarded_args: &[String]) -> i32 {
    plugin_cli::run(forwarded_args).await
}

/// Handle `caliban router debug ...`. Prints diagnostics to stdout and
/// returns. The caller should treat this as an early exit.
pub(crate) fn run_router_debug(
    cmd: &RouterCommand,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    match cmd {
        RouterCommand::Debug(dbg) => {
            let cwd = std::env::current_dir().context("could not get cwd")?;
            let out = router::run_debug(dbg, config_path, &cwd)?;
            print!("{out}");
            Ok(())
        }
    }
}

/// Dispatch the ADR 0037 supervisor subcommands (`agents`, `daemon`,
/// `attach`, `logs`, `stop`, `kill`, `respawn`, `rm`). Returns the
/// supervisor exit code, or `None` for `CalibanCommand::Router` (which
/// is handled by [`run_router_debug`]).
pub(crate) async fn run_supervisor_command(cmd: &CalibanCommand) -> Option<i32> {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[caliban] could not get cwd: {e}");
            return Some(1);
        }
    };
    let repo = agents_cli::discover_repo_root(&cwd);
    match cmd {
        CalibanCommand::Agents { inner } => Some(agents_cli::run_agents(inner, &repo).await),
        CalibanCommand::Daemon { inner } => Some(agents_cli::run_daemon(inner, &repo).await),
        CalibanCommand::Attach { id } => {
            Some(agents_cli::run_agents(&AgentsCommand::Attach { id: id.clone() }, &repo).await)
        }
        CalibanCommand::Logs { id } => {
            Some(agents_cli::run_agents(&AgentsCommand::Logs { id: id.clone() }, &repo).await)
        }
        CalibanCommand::Stop { id } | CalibanCommand::Kill { id } => {
            Some(agents_cli::run_agents(&AgentsCommand::Kill { id: id.clone() }, &repo).await)
        }
        CalibanCommand::Respawn { id } => {
            Some(agents_cli::run_agents(&AgentsCommand::Respawn { id: id.clone() }, &repo).await)
        }
        CalibanCommand::Rm { id, force } => Some(
            agents_cli::run_agents(
                &AgentsCommand::Rm {
                    id: id.clone(),
                    force: *force,
                },
                &repo,
            )
            .await,
        ),
        CalibanCommand::Router { .. } => None,
    }
}

/// Handle the top-level `--bg <TASK>` shortcut. Asks the per-repo
/// supervisor daemon (auto-spawned if needed) to register a new
/// background agent and returns its exit code (ADR 0037).
pub(crate) async fn run_bg_shortcut(task: &str) -> Result<i32> {
    let cwd = std::env::current_dir().context("could not get cwd")?;
    let repo = agents_cli::discover_repo_root(&cwd);
    Ok(agents_cli::run_bg(task, &repo).await)
}
