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
            // Router debug resolves config the same way a real run does: an
            // explicit --config file wins, else the settings-layer [router]
            // section (#699 — discovery removed).
            let workspace = std::env::current_dir().context("could not get cwd")?;
            let mut opts = caliban_settings::LoadOptions::new(workspace);
            opts.bare = false;
            let outcome = caliban_settings::load_settings(&opts)
                .map_err(|e| anyhow::anyhow!(e))
                .context("load layered settings")?;
            let out = router::run_debug(dbg, config_path, outcome.settings.router.as_ref())?;
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
        | CalibanCommand::Mcp { .. }
        | CalibanCommand::Http { .. }
        | CalibanCommand::Acp { .. }
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

/// Build the `caliban config print` JSON envelope: the merged settings, the
/// per-scope `_sources`, and the per-key `_provenance` map. The loader computes
/// true per-key provenance (#411) and its own doc says `config print` uses it,
/// but Print previously discarded it and emitted only the flat `_sources` list
/// (#620).
fn config_print_envelope(outcome: &caliban_settings::LoadOutcome) -> Result<serde_json::Value> {
    let settings_json = serde_json::to_value(&outcome.settings).context("serialize Settings")?;
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
    let provenance_json: serde_json::Map<String, serde_json::Value> = outcome
        .provenance
        .iter()
        .map(|(k, scope)| {
            (
                k.clone(),
                serde_json::Value::String(scope.label().to_string()),
            )
        })
        .collect();
    let env_overrides_json: Vec<_> = outcome
        .env_overrides
        .iter()
        .map(|o| {
            serde_json::json!({
                "key": o.key_path,
                "env": o.env_var,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "settings": settings_json,
        "_sources": sources_json,
        "_provenance": provenance_json,
        // Env-sourced overrides that won over the file (#538): each names the
        // settings key and the CALIBAN_* variable that set it.
        "_env_overrides": env_overrides_json,
    }))
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
            let envelope = config_print_envelope(&outcome).context("serialize Settings")?;
            println!("{}", serde_json::to_string_pretty(&envelope)?);
            Ok(0)
        }
        ConfigCommand::Migrate { dry_run } => {
            // `outcome.settings` is the fully-resolved config and already has
            // the legacy TOMLs folded in by the startup compat shim — so it is
            // the correct migration *output*. But that also means the
            // `maybe_load_legacy_*` guards (which no-op when the target already
            // has rules) always trip against it, so we can't use them to detect
            // *which* legacy files were present. Probe a fresh, empty Settings
            // instead: its empty state passes the guards, so a `true` return
            // means that legacy file exists and contributed (#176).
            let migrated = outcome.settings.clone();
            let mut probe = caliban_settings::Settings::default();
            let mut touched = Vec::new();
            if caliban_settings::compat::maybe_load_legacy_mcp(&mut probe, &workspace) {
                touched.push("mcp.toml → settings.mcp_servers");
            }
            // `maybe_load_legacy_permissions` can't be used as a presence
            // probe: its `load_rules()` always appends the built-in
            // `default_rules()` tail, so it reports "found" even with no legacy
            // file. `legacy_permissions_present` excludes that tail (#176).
            if caliban_settings::compat::legacy_permissions_present(&workspace) {
                touched.push("permissions.toml → settings.permissions");
            }
            if caliban_settings::compat::maybe_load_legacy_hooks(&mut probe, &workspace) {
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
        ConfigCommand::ImportRouter { from, dry_run } => {
            run_import_router(&workspace, from.as_deref(), *dry_run)
        }
    }
}

/// Handle `caliban config import-router` (#699): migrate a legacy `caliban.toml`
/// router config into `<workspace>/.caliban/settings.toml` `[router]` (moving
/// top-level `[provider.X]` blocks under `[router.provider.X]`). Existing keys
/// in `settings.toml` are preserved; only `[router]` is replaced.
fn run_import_router(
    workspace: &std::path::Path,
    from: Option<&std::path::Path>,
    dry_run: bool,
) -> Result<i32> {
    // Resolve the source caliban.toml: explicit --from, else the nearest one.
    let source = match from {
        Some(p) => p.to_path_buf(),
        None => caliban_common::paths::walk_up_for_file(workspace, "caliban.toml")
            .context("no caliban.toml found to migrate (pass --from <PATH>)")?,
    };
    let body = std::fs::read_to_string(&source)
        .with_context(|| format!("reading {}", source.display()))?;
    let router_value = router::caliban_toml_to_settings_router(&body)
        .with_context(|| format!("migrating {}", source.display()))?;

    // Merge into <workspace>/.caliban/settings.toml, preserving other keys.
    // Round-trip through the Settings type (which captures unknown keys in its
    // `extra` flatten field) so the serializer emits tables in a valid order —
    // a hand-merged toml::Table can hit TOML's "value after table" error when an
    // existing scalar key sorts after `[router]`.
    let dest_dir = workspace.join(".caliban");
    let dest = dest_dir.join("settings.toml");
    let mut settings: caliban_settings::Settings = if dest.is_file() {
        toml::from_str(
            &std::fs::read_to_string(&dest)
                .with_context(|| format!("reading {}", dest.display()))?,
        )
        .with_context(|| format!("parsing existing {}", dest.display()))?
    } else {
        caliban_settings::Settings::default()
    };
    settings.router = Some(router_value);
    let serialized = toml::to_string_pretty(&settings).context("serialize settings.toml")?;

    if dry_run {
        println!("{serialized}");
        eprintln!(
            "[caliban] dry-run; would migrate {} → {} [router]",
            source.display(),
            dest.display()
        );
        return Ok(0);
    }
    std::fs::create_dir_all(&dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    std::fs::write(&dest, serialized).with_context(|| format!("write {}", dest.display()))?;
    println!(
        "migrated {} → {} [router]",
        source.display(),
        dest.display()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn config_print_envelope_surfaces_per_key_provenance() {
        // #620: Print must include the per-key `_provenance` the loader computes,
        // not just the flat `_sources` list.
        let outcome = caliban_settings::LoadOutcome {
            settings: caliban_settings::Settings::default(),
            sources: Vec::new(),
            provenance: BTreeMap::from([
                ("model".to_string(), caliban_settings::Scope::User),
                ("max_tokens".to_string(), caliban_settings::Scope::Project),
            ]),
            validation_warnings: Vec::new(),
            env_overrides: Vec::new(),
        };
        let env = config_print_envelope(&outcome).expect("envelope builds");
        let prov = env
            .get("_provenance")
            .and_then(|v| v.as_object())
            .expect("_provenance object present");
        assert_eq!(
            prov.get("model").and_then(|v| v.as_str()),
            Some(caliban_settings::Scope::User.label())
        );
        assert_eq!(
            prov.get("max_tokens").and_then(|v| v.as_str()),
            Some(caliban_settings::Scope::Project.label())
        );
        // The existing surfaces are still present.
        assert!(env.get("settings").is_some());
        assert!(env.get("_sources").is_some());
    }

    const CALIBAN_TOML: &str = r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "openai"
model = "local"

[provider.openai]
base_url = "http://localhost:8080/v1"
"#;

    #[test]
    fn import_router_merges_into_settings_preserving_keys() {
        // #699: `config import-router` migrates a caliban.toml into the settings
        // [router] section (nesting [provider.X] under [router.provider.X]) while
        // preserving other settings keys and producing valid TOML.
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::write(ws.join("caliban.toml"), CALIBAN_TOML).unwrap();
        std::fs::create_dir_all(ws.join(".caliban")).unwrap();
        // A pre-existing scalar key (`view_mode`) that sorts AFTER `router`,
        // which is exactly the TOML "value after table" ordering hazard.
        std::fs::write(
            ws.join(".caliban/settings.toml"),
            "model = \"claude-opus-4-8\"\nview_mode = \"compact\"\n",
        )
        .unwrap();

        assert_eq!(run_import_router(ws, None, false).unwrap(), 0);

        let written = std::fs::read_to_string(ws.join(".caliban/settings.toml")).unwrap();
        let settings: caliban_settings::Settings = toml::from_str(&written).unwrap();
        let router = settings.router.expect("router migrated");
        assert_eq!(
            router
                .get("provider")
                .and_then(|p| p.get("openai"))
                .and_then(|o| o.get("base_url"))
                .and_then(|u| u.as_str()),
            Some("http://localhost:8080/v1"),
            "provider block relocated under router.provider"
        );
        // Pre-existing keys survive the merge.
        assert!(settings.model.is_some(), "pre-existing model preserved");
        assert_eq!(settings.view_mode.as_deref(), Some("compact"));
    }

    #[test]
    fn import_router_dry_run_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::write(ws.join("caliban.toml"), CALIBAN_TOML).unwrap();
        assert_eq!(run_import_router(ws, None, true).unwrap(), 0);
        assert!(
            !ws.join(".caliban/settings.toml").exists(),
            "dry-run must not write settings.toml"
        );
    }
}
