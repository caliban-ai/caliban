//! caliban — agent harness binary.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::multiple_crate_versions)]

mod agents_cli;
mod args;
mod attach;
mod diagnostics;
mod effective_model;
mod headless;
mod hook_inherit;
mod perms_cli;
mod plugin_cli;
mod provider_wiring;
mod refreshing_provider;
mod router;
mod settings_cli;
mod startup;
mod stream_decode;
mod subcommands;
mod system_prompt;
mod tui;
mod version;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use caliban_provider::Provider;
use caliban_tools_builtin::WorkspaceRoot;
use clap::Parser;
use tokio_util::sync::CancellationToken;

// Re-export the CLI types at the crate root so the existing
// `crate::Args` / `crate::ProviderKind` / `crate::AgentsCommand` /
// `crate::default_model_for` references from `tui.rs`, `agents_cli.rs`,
// and the headless / system_prompt modules keep working after the split.
#[allow(unused_imports)]
pub(crate) use crate::args::{
    AgentsCommand, Args, CalibanCommand, DaemonCommand, PermsCommand, ProviderKind, RouterCommand,
    SettingsCommand, default_model_for, provider_name, resolved_provider,
};

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<()> {
    use std::io::IsTerminal as _;

    let mut args = Args::parse();

    // Cross-flag validation that clap can't express natively. Fail loud
    // with EX_USAGE (64) so operators don't debug silent prompt-bypass
    // bugs (lmstudio Finding 13).
    if let Err(e) = crate::args::validate_stream_json_input(&args) {
        eprintln!("[caliban] {e}");
        std::process::exit(64);
    }

    // Internal worker entry (ADR 0037, #71). Dispatched first: it builds
    // its own agent and must not run the normal startup path.
    if let Some(CalibanCommand::AgentWorker {
        manifest,
        socket,
        listen,
        control_socket,
    }) = &args.command
    {
        let code = worker::run(
            manifest,
            socket.as_deref(),
            listen.as_deref(),
            control_socket.as_deref(),
        )
        .await;
        std::process::exit(code);
    }

    // `caliban plugin <verb> ...` — plugin manager (ADR 0030). The clap
    // declaration uses `trailing_var_arg`, so the plugin CLI parses
    // its own verbs directly. Dispatched ahead of provider construction
    // so plugin management works even when auth/network is broken.
    if let Some(CalibanCommand::Plugin { args: plugin_args }) = &args.command {
        let code = subcommands::run_plugin_cli(plugin_args).await;
        std::process::exit(code);
    }

    // Diagnostic subcommands run before any provider construction or hook
    // wiring — they only need to read config.
    if let Some(CalibanCommand::Router { inner }) = &args.command {
        subcommands::run_router_debug(inner, args.config_path.as_deref())?;
        return Ok(());
    }

    // `caliban doctor [--deep]` — run health checks and exit with
    // status 1 if anything failed, else 0. Wired ahead of provider
    // construction so it runs even when auth/network is broken.
    if let Some(CalibanCommand::Doctor { deep }) = &args.command {
        let diag = diagnostics::Diagnostics::run(diagnostics::DiagOpts {
            deep: *deep,
            model: args.model.clone(),
        })
        .await;
        diagnostics::print_diagnostics_text(&diag);
        std::process::exit(diag.exit_code());
    }

    // `caliban config <verb>` — settings inspection / migration. No
    // provider or daemon needed (ADR 0026).
    if let Some(CalibanCommand::Config { inner }) = &args.command {
        let code = subcommands::run_config(inner)?;
        std::process::exit(code);
    }

    // `caliban perms <verb>` — permission rule management (ADR 0029 / Phase 6).
    if let Some(CalibanCommand::Perms { cmd }) = &args.command {
        let code = perms_cli::run(cmd);
        std::process::exit(code);
    }

    // `caliban settings <verb>` — settings import / print (Phase 6).
    if let Some(CalibanCommand::Settings { cmd }) = &args.command {
        let code = settings_cli::run(cmd);
        std::process::exit(code);
    }

    // ADR 0037 subcommands. They auto-spawn the supervisor daemon as needed
    // and don't require a provider, so route them first.
    if let Some(cmd) = &args.command
        && let Some(code) = subcommands::run_supervisor_command(cmd).await
    {
        std::process::exit(code);
    }

    // Top-level --bg shortcut.
    if let Some(task) = &args.bg {
        let code = subcommands::run_bg_shortcut(task).await?;
        std::process::exit(code);
    }

    // Initialize telemetry (cost ledger + optional OTLP span export). The real
    // exporter pipeline is built here when CALIBAN_ENABLE_TELEMETRY is set and
    // the OTEL_* contract points at a collector; otherwise this is a cheap,
    // disabled handle. Held for the life of the process and force-flushed on
    // exit. Requires the Tokio runtime (batch span processor), which
    // `#[tokio::main]` provides.
    let telemetry_session_id = args
        .session
        .clone()
        .or_else(|| args.resume.clone())
        .unwrap_or_else(caliban_telemetry::Telemetry::new_session_id);
    let telemetry = caliban_telemetry::Telemetry::init_from_env(&telemetry_session_id)
        .context("initialize telemetry")?;

    // Install the global tracing subscriber: file-backed fmt layer when --debug
    // / CALIBAN_DEBUG is set, plus the OTLP span-export layer when telemetry is
    // enabled. No-op when neither applies.
    startup::init_tracing(&args, &telemetry).await;

    let workspace = match &args.workspace {
        Some(p) => {
            // Fail-fast if the path is bogus rather than deferring to
            // the first tool call. Exit 64 (`EX_USAGE`) per ADR 0025.
            match std::fs::metadata(p) {
                Ok(m) if m.is_dir() => {}
                Ok(_) => {
                    eprintln!("[caliban] --workspace {}: not a directory", p.display());
                    std::process::exit(64);
                }
                Err(e) => {
                    eprintln!("[caliban] --workspace {}: {e}", p.display());
                    std::process::exit(64);
                }
            }
            WorkspaceRoot::new(p.clone())
        }
        None => WorkspaceRoot::current_dir().context("could not get cwd")?,
    };

    // Load layered settings (ADR 0026). `--bare` mode short-circuits.
    // Parse / CLI-overlay / unknown-scope failures are fatal with
    // EX_CONFIGURATION_ERROR (78) — see ADR 0025's exit-code table.
    // IO errors on a single scope file still abort; the loader returns
    // an error rather than degrading silently.
    let settings_outcome = match startup::load_layered_settings(&args, workspace.root()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[caliban] {e}");
            std::process::exit(78);
        }
    };
    for w in &settings_outcome.validation_warnings {
        tracing::warn!(target: caliban_common::tracing_targets::TARGET_SETTINGS, warning = %w, "settings schema validation");
    }
    let settings_handle = caliban_settings::SettingsHandle::new(settings_outcome.settings.clone());
    let _settings_sources = settings_outcome.sources.clone();
    let _ = settings_handle.current(); // touch to ensure the handle is connected
    let settings_snapshot = settings_outcome.settings.clone();

    // Resolve effective (provider, model) from CLI > Settings > builtin
    // default, then mutate `args` so every downstream site that reads
    // `args.provider` / `args.model` sees the precedence-resolved value.
    // The `EffectiveModel` itself carries provenance for `/config` and
    // `caliban doctor`.
    let effective = crate::effective_model::EffectiveModel::resolve(&args, &settings_snapshot)
        .context("resolving effective model from CLI args + settings")?;
    args.provider = Some(effective.provider);
    if args.model.is_none() {
        args.model = Some(effective.name.clone());
    }
    if args.fallback_model.is_none() {
        args.fallback_model = effective.fallback.as_ref().map(|(_, n)| n.clone());
    }
    tracing::info!(
        target: "caliban::config",
        provider = ?effective.provider,
        model = %effective.name,
        source = effective.source.label(),
        "effective model resolved",
    );

    // ApiKeyHelperPool — built once from settings. Consumed by provider
    // construction (both single-provider and router paths) and by
    // `RefreshingProvider` for on-401 re-acquisition. Empty pool ⇒
    // `has_spec_for(...)` returns false everywhere and the env-var path
    // runs exactly as before.
    let helper_pool = std::sync::Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(
        settings_snapshot.api_key_helper.as_ref(),
    ));

    // Honor `enable_telemetry` from settings when the env override is
    // unset.
    if settings_snapshot.enable_telemetry == Some(true)
        && std::env::var("CALIBAN_ENABLE_TELEMETRY").is_err()
    {
        tracing::info!(target: caliban_common::tracing_targets::TARGET_SETTINGS, "telemetry enabled via settings.json");
    }

    // Pre-flight (pre-driver) fatal errors — provider construction, a missing
    // API key, model pre-check, etc. — must reach structured consumers as an
    // NDJSON/JSON `result` frame, not leak as a plain `Error:` line from the
    // top-level handler before the headless driver ever starts (#429). Compute
    // whether a structured output format is active, and route every pre-driver
    // fallible step through `preflight!`.
    let preflight_fmt = args.output_format.unwrap_or(headless::OutputFormat::Text);
    let preflight_structured = {
        use std::io::IsTerminal as _;
        args::is_headless_active(
            &args,
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        ) && matches!(
            preflight_fmt,
            headless::OutputFormat::StreamJson | headless::OutputFormat::Json
        )
    };
    macro_rules! preflight {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(err) => {
                    let e: anyhow::Error = err.into();
                    if preflight_structured {
                        headless::emit_preflight_error(preflight_fmt, &format!("{e:#}"));
                        std::process::exit(1);
                    }
                    return Err(e);
                }
            }
        };
    }

    // Router v2: try caliban.toml first (--config flag or discovery), fall
    // back to the single-provider construction when no router config is
    // present (preserving v1 behavior). ADR 0038.
    let cwd_for_router = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let provider: Arc<dyn Provider + Send + Sync> =
        preflight!((|| -> anyhow::Result<Arc<dyn Provider + Send + Sync>> {
            Ok(
                match router::try_load(args.config_path.as_deref(), &cwd_for_router, &helper_pool)?
                {
                    Some(wiring) => {
                        tracing::info!(
                            target: caliban_common::tracing_targets::TARGET_ROUTER,
                            path = %wiring.config_path.display(),
                            routes = wiring.router.routes().len(),
                            "model router wired from caliban.toml",
                        );
                        wiring.router
                    }
                    None => startup::build_provider(&args, &helper_pool)?,
                },
            )
        })());
    let todos = caliban_agent_core::new_shared_todos();
    let plan_mode = caliban_agent_core::new_shared_plan_mode();

    // Enforce gate: when permissions.enforce = true, refuse bypass flags.
    preflight!(
        startup::check_enforce_gate(&args, &settings_snapshot).map_err(|e| anyhow::anyhow!(e))
    );

    // Resolve the initial permission mode (ADR 0029). CLI flag wins over
    // env; bypass mode requires --allow-dangerously-skip-permissions.
    let env_perm = std::env::var("CALIBAN_DEFAULT_PERMISSION_MODE").ok();
    let initial_perm_mode = preflight!(
        caliban_agent_core::resolve_startup_mode(
            args.permission_mode.as_deref(),
            env_perm.as_deref(),
            settings_snapshot.permissions.default_mode.as_deref(),
            args.allow_dangerously_skip_permissions,
        )
        .map_err(|e| anyhow::anyhow!(e))
    );
    let permission_mode = caliban_agent_core::SharedPermissionMode::new(initial_perm_mode);
    // Keep the legacy plan-mode flag in sync with `Plan`.
    if initial_perm_mode == caliban_agent_core::PermissionMode::Plan {
        plan_mode.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    // Discover plugins early (ADR 0030). Plugins contribute skill roots,
    // hooks config, MCP servers, agents, and output styles, so the manager
    // is constructed before any of those subsystems init.
    let plugin_manager = startup::load_plugin_manager(&args, workspace.root());
    let plugin_skill_roots = plugin_manager.skill_roots();
    // Build the plugin descriptors that surface in the headless
    // `system/init` frame. Empty when `--bare` / `--no-plugins`.
    let plugin_descriptors: Vec<serde_json::Value> = plugin_manager
        .loaded()
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.manifest.name,
                "version": p.manifest.version,
                "source": p.source.as_str(),
            })
        })
        .collect();

    let mut registry = startup::build_registry(
        &args,
        workspace,
        Arc::clone(&todos),
        Arc::clone(&plan_mode),
        &plugin_skill_roots,
    );

    // MCP servers — Phase B: stdio + HTTP + SSE transports (ADR 0023).
    // Retains the parsed `McpConfig.servers` map so the permissions setup
    // downstream can fold `[server.X.permissions]` blocks into the global
    // rule list. MCP servers come from the unified Settings snapshot
    // (caliban-settings already folds legacy `mcp.toml` via its compat
    // shim).
    // Whether this run may perform interactive OAuth authorization for
    // `oauth = auto|manual` MCP servers (open a browser + block on the loopback
    // callback). Only true for an interactive run — i.e. NOT headless/`--print`
    // /stream-json/non-TTY. Mirrors the headless-dispatch decision made below;
    // computed here because MCP starts before that dispatch.
    let mcp_interactive = {
        use std::io::IsTerminal as _;
        !args::is_headless_active(
            &args,
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        )
    };
    let (mcp_summaries, mcp_server_cfg, mcp_tools_for_search) =
        startup::start_mcp(&args, &settings_snapshot, &mut registry, mcp_interactive).await;

    // ADR-0046 — set up the shared MCP activation surface for lazy
    // schema loading. The Arc<ArcSwap<McpActivationSet>> must be the
    // same instance held by both ToolSearchTool and the Agent so the
    // model's activations propagate into the per-turn wire filter.
    let mcp_eager_servers = Arc::new(startup::compute_mcp_eager_servers(&mcp_server_cfg));
    let max_active_schemas = settings_snapshot
        .tools
        .as_ref()
        .and_then(|t| t.max_active_schemas)
        .unwrap_or(24);
    let mcp_active = Arc::new(arc_swap::ArcSwap::from_pointee(
        caliban_agent_core::mcp_activation::McpActivationSet::new(max_active_schemas),
    ));
    // Register ToolSearch BEFORE install_sub_agent so sub-agents
    // inherit the discovery tool in their registry snapshot.
    startup::install_tool_search(
        &args,
        &mut registry,
        mcp_tools_for_search,
        Arc::clone(&mcp_active),
    );

    let model = args
        .model
        .clone()
        .unwrap_or_else(|| default_model_for(resolved_provider(&args)).to_string());

    // F4 pre-flight: when targeting a non-canonical OpenAI endpoint
    // (LM Studio etc.), confirm the model is loaded *before* the agent
    // loop fires its first request. Local servers silently substitute
    // the first loaded model for unknown IDs, so a typo runs the wrong
    // model with no visible signal. The check is a no-op for canonical
    // OpenAI / Anthropic / Google / Ollama.
    preflight!(startup::preflight_model_check(&args, &model).await);

    // Project hooks config out of the layered Settings snapshot (ADR 0026).
    // The in-process PermissionsHook still runs even when --no-hooks /
    // --bare are set; the legacy `hooks.toml` loader is reachable through
    // the `caliban-settings::compat` shim during the back-compat window.
    let hooks_cfg = startup::load_hooks_config(&args, &settings_snapshot);
    // The summary count includes the legacy-compat handler count when
    // settings.json was silent and hooks.toml was loaded via the compat
    // shim (preserved via `Settings::legacy_hook_handler_count`).
    let total_handlers = hooks_cfg.total_handler_count()
        + settings_snapshot.legacy_hook_handler_count().unwrap_or(0);
    let hooks_cfg_summary = (total_handlers, hooks_cfg.disable_all_hooks || args.no_hooks);

    // Decide whether the TUI is the active mode (and therefore should provide
    // the interactive Ask modal).
    let tui_mode_active = {
        use std::io::IsTerminal as _;
        let has_prompt = args.prompt.is_some() || args.prompt_flag.is_some();
        let headless_explicit = args.print.is_some() || args.output_format.is_some();
        let auto_headless = !args.no_auto_print
            && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal());
        let headless_active = headless_explicit || (has_prompt && auto_headless);
        !has_prompt && !headless_active && std::io::stdin().is_terminal()
    };

    let startup::PermissionsSetup {
        permissions_hook,
        tui_ask_rx,
        auto_mode_classifier,
        runtime_rules,
        inheritable_config,
    } = startup::build_permissions(
        &args,
        &settings_snapshot,
        &mcp_server_cfg,
        &provider,
        &model,
        &permission_mode,
        tui_mode_active,
    );

    // Wire AgentTool (sub-agent primitive) — closes over a snapshot of
    // the registry so sub-agents cannot recurse. Background-handoff
    // spawner asks the per-repo supervisor daemon to register new agents
    // (ADR 0037). The resolved permission config is threaded in so the
    // bg_spawner can stamp it into SpawnSpec when `inherit_hooks=true` (#84).
    startup::install_sub_agent(
        &args,
        &mut registry,
        &provider,
        &model,
        Arc::clone(&mcp_active),
        Arc::clone(&mcp_eager_servers),
        max_active_schemas,
        settings_snapshot
            .tools
            .as_ref()
            .and_then(|t| t.lazy_mcp)
            .unwrap_or(false),
        inheritable_config,
        Arc::clone(&runtime_rules),
    );

    // When `--include-hook-events` is set, allocate a buffer so the
    // `HeadlessHookSink` can capture every emitted event for the
    // stream-json driver to flush (ADR 0025). Headless-only.
    let hook_event_buffer = if args.include_hook_events {
        Some(headless::new_event_buffer())
    } else {
        None
    };

    let agent = preflight!(startup::build_agent(
        &args,
        provider,
        registry,
        &model,
        &plan_mode,
        permissions_hook,
        hook_event_buffer.as_ref(),
        &settings_snapshot,
        &hooks_cfg,
        Arc::clone(&mcp_active),
        Arc::clone(&mcp_eager_servers),
    ));

    // Fire SessionStart hook (best-effort); collect any hook-supplied context
    // to splice into the system prompt before turn 1 (#106).
    let session_context = startup::fire_session_start(&args, &agent, &model).await;
    let _ = hooks_cfg_summary; // silence unused when not later consumed

    // Resolve session store + persisted session (when --session is given).
    // Seeds the shared todos handle and plan-mode flag from the snapshot.
    let (store, mut session) =
        preflight!(startup::resolve_session(&args, &model, &todos, &plan_mode));

    // Resolve system prompt from CLI flags (or build default), then layer
    // the active output-style block + memory-tier prefix when the default
    // is in effect.
    let cwd_for_prompt = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let system_prompt = preflight!(
        startup::resolve_system_prompt(
            &args,
            &agent,
            &cwd_for_prompt,
            &settings_snapshot,
            &session_context,
        )
        .await
    );

    // Snapshot todos for splicing into the system prompt for this run.
    let todo_snapshot = todos.lock().expect("todos lock poisoned").clone();

    // For fresh sessions (no prior messages), insert the system prompt at position 0
    // with the current todos appended.
    if let Some(sess) = session.as_mut()
        && sess.messages.is_empty()
        && let Some(ref prompt) = system_prompt
    {
        let with_todos = system_prompt::append_todo_block(prompt, &todo_snapshot);
        sess.messages
            .push(caliban_provider::Message::system_text(with_todos));
    } else if let Some(sess) = session.as_mut()
        && !sess.messages.is_empty()
        && sess.messages[0].role == caliban_provider::Role::System
        && let Some(ref prompt) = system_prompt
    {
        // Existing session with a system message at position 0: rebuild it so
        // the latest todo snapshot is reflected.
        let with_todos = system_prompt::append_todo_block(prompt, &todo_snapshot);
        sess.messages[0] = caliban_provider::Message::system_text(with_todos);
    }

    // --- Headless / print-mode dispatch (ADR 0025). See
    // `args::is_headless_active` for the trigger rules (explicit print /
    // output-format, stream-json input, or auto-headless with a prompt).
    let has_prompt = args.prompt.is_some() || args.prompt_flag.is_some();
    let headless_active = {
        use std::io::IsTerminal as _;
        args::is_headless_active(
            &args,
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        )
    };
    if headless_active {
        let cancel = CancellationToken::new();
        {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                cancel.cancel();
                let _ = tokio::signal::ctrl_c().await;
                std::process::exit(130);
            });
        }
        let resolved_perm_mode = permission_mode.load();
        let exit = startup::run_headless(
            &args,
            agent,
            system_prompt,
            todo_snapshot,
            session,
            store,
            todos,
            plan_mode,
            model,
            cancel,
            hook_event_buffer,
            plugin_descriptors,
            resolved_perm_mode,
        )
        .await;
        // Force-flush batched OTLP spans + session-end metric before exit.
        let _ = telemetry.shutdown();
        std::process::exit(exit);
    }
    // hook_event_buffer is consumed by headless mode; for the TUI/interactive
    // path the buffer is dropped here (the sink still runs but its frames
    // are unused — informational).
    drop(hook_event_buffer);

    // --- TUI dispatch: no prompt + stdin is a TTY → enter interactive TUI.
    let stdin_is_tty = std::io::stdin().is_terminal();
    if !has_prompt {
        if stdin_is_tty {
            let bypass_latch = args.allow_dangerously_skip_permissions;
            let settings_sources_view: Vec<(String, Option<PathBuf>, Option<String>)> =
                settings_outcome
                    .sources
                    .iter()
                    .map(|s| {
                        (
                            s.scope.label().to_string(),
                            s.path.clone(),
                            s.format.map(str::to_string),
                        )
                    })
                    .collect();
            let result = tui::run(
                args,
                agent,
                store,
                session,
                system_prompt,
                todos,
                plan_mode,
                permission_mode,
                bypass_latch,
                auto_mode_classifier,
                mcp_summaries,
                tui_ask_rx,
                Some(settings_handle.clone()),
                settings_sources_view,
                runtime_rules,
            )
            .await;
            // Force-flush batched OTLP spans + session-end metric before exit.
            let _ = telemetry.shutdown();
            return result;
        }
        anyhow::bail!(
            "no prompt given and stdin is not a TTY; use --prompt or pass a positional argument"
        );
    }

    // --- Single-prompt path: register the outer Ctrl-C handler, drive
    // the agent loop, fire SessionEnd, and optionally persist the
    // session back to disk.
    let result = startup::run_single_prompt(
        &args,
        agent,
        system_prompt,
        todo_snapshot,
        session,
        store,
        todos,
        plan_mode,
        model,
    )
    .await;
    // Force-flush batched OTLP spans + session-end metric before exit.
    let _ = telemetry.shutdown();
    result
}
