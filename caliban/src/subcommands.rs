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
use crate::args::{AgentsCommand, CalibanCommand, ConfigCommand, RouterCommand};
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
        // `caliban router` / `doctor` / `config` / `plugin` / `perms` /
        // `settings` are dispatched in main.rs ahead of the supervisor
        // entry points (no supervisor needed for diagnostics, config
        // inspection, permission management, or plugin management). Skip
        // them here so we don't accidentally spawn the daemon.
        CalibanCommand::Router { .. }
        | CalibanCommand::Doctor { .. }
        | CalibanCommand::Config { .. }
        | CalibanCommand::Plugin { .. }
        | CalibanCommand::Perms { .. }
        | CalibanCommand::Settings { .. }
        | CalibanCommand::AgentWorker { .. } => None,
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

/// Handle `caliban config <verb>` (ADR 0026). Reads the layered
/// settings, then either prints them or migrates legacy per-feature
/// TOMLs into the project-scope `settings.json`.
pub(crate) fn run_config(cmd: &ConfigCommand) -> Result<i32> {
    let workspace = std::env::current_dir().context("could not get cwd")?;
    let mut opts = caliban_settings::LoadOptions::new(workspace.clone());
    // `print` and `migrate` both reflect what would *actually* load in
    // a normal run, so we don't override scope_filter / overlay here.
    opts.bare = false;
    let outcome = caliban_settings::load_settings(&opts)
        .map_err(|e| anyhow::anyhow!(e))
        .context("load layered settings")?;
    match cmd {
        ConfigCommand::Print => {
            // Emit the merged Settings as pretty JSON plus a comment-
            // free `_sources` array recording where each scope file
            // lived.
            let settings_json =
                serde_json::to_value(&outcome.settings).context("serialize Settings")?;
            let sources_json: Vec<_> = outcome
                .sources
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "scope": s.scope.label(),
                        "path": s.path,
                        "format": s.format,
                    })
                })
                .collect();
            let envelope = serde_json::json!({
                "settings": settings_json,
                "_sources": sources_json,
            });
            println!("{}", serde_json::to_string_pretty(&envelope)?);
            Ok(0)
        }
        ConfigCommand::Migrate { dry_run } => {
            let mut migrated = outcome.settings.clone();
            let mut touched = Vec::new();
            if caliban_settings::compat::maybe_load_legacy_mcp(&mut migrated, &workspace) {
                touched.push("mcp.toml → settings.mcp_servers");
            }
            if caliban_settings::compat::maybe_load_legacy_permissions(&mut migrated, &workspace) {
                touched.push("permissions.toml → settings.permissions");
            }
            if caliban_settings::compat::maybe_load_legacy_hooks(&mut migrated, &workspace) {
                touched.push("hooks.toml → settings.hooks");
            }
            if touched.is_empty() {
                eprintln!("[caliban] no legacy TOMLs to migrate (already on settings.json)");
                return Ok(0);
            }
            let serialized =
                serde_json::to_string_pretty(&migrated).context("serialize migrated Settings")?;
            if *dry_run {
                println!("{serialized}");
                eprintln!("[caliban] dry-run; would migrate: {}", touched.join(", "));
                return Ok(0);
            }
            let dest_dir = workspace.join(".caliban");
            std::fs::create_dir_all(&dest_dir)
                .with_context(|| format!("create {}", dest_dir.display()))?;
            let dest = dest_dir.join("settings.json");
            std::fs::write(&dest, serialized)
                .with_context(|| format!("write {}", dest.display()))?;
            println!("migrated to {}: {}", dest.display(), touched.join(", "));
            Ok(0)
        }
    }
}
