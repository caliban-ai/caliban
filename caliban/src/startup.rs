//! State assembly for the `caliban` binary.
//!
//! Hosts every helper that constructs runtime state shared across the
//! three dispatch paths (TUI / headless / single-prompt):
//!
//! - [`init_debug_tracing`] — file-backed `tracing` subscriber.
//! - [`build_provider`] — single-provider construction (router fallback).
//! - [`web_fetch_client`] — `reqwest::Client` for `WebFetchTool`.
//! - [`build_registry`] — registry assembly (built-in tools + plugins).
//! - [`load_layered_settings`] — ADR 0026 `settings.json` loader.
//! - [`auto_memory_disabled`] — `CALIBAN_DISABLE_AUTO_MEMORY` check.
//! - [`run_and_render`] — single-prompt agent driver.
//! - [`run_headless`] — `-p` / `--print` agent driver.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use caliban_agent_core::{Agent, Message, ToolRegistry};
use caliban_provider::{Provider, Usage};
use caliban_sessions::{PersistedSession, SessionStore};
use caliban_skills::{SkillTool, load_skills, register_builtins};
use caliban_tools_builtin::{
    AgentFactory, AgentTool, AgentToolInput, BashOutputTool, BashTool, EditTool, EnterPlanModeTool,
    ExitPlanModeTool, GlobTool, GrepTool, KillShellTool, MultiEditTool, NotebookEditTool,
    ReadMemoryTopicTool, ReadTool, TodoWriteTool, WebFetchTool, WebSearchTool, WorkspaceRoot,
    WriteMemoryTopicTool, WriteTool,
};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

use crate::agents_cli;
use crate::args::{Args, ProviderKind, provider_name, summarize, summarize_blocks};
use crate::{headless, system_prompt, tui};

/// Install a file-backed `tracing` subscriber when `--debug` or
/// `CALIBAN_DEBUG` is set. No-op otherwise. Idempotent once initialized:
/// runs at most once at startup before any `tracing::*!` site fires.
pub(crate) async fn init_debug_tracing(args: &Args) {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let debug = args.debug || std::env::var("CALIBAN_DEBUG").is_ok();
    if !debug {
        return;
    }
    let Some(log_path) = dirs::cache_dir().map(|d| d.join("caliban").join("debug.log")) else {
        return;
    };
    if let Some(parent) = log_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let opened = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await;
    let Ok(async_file) = opened else { return };
    // tracing-subscriber's fmt layer wants std::io::Write, so
    // convert back to a std::fs::File. into_std offloads to the
    // blocking pool; safe here since this only runs once at start.
    let file = async_file.into_std().await;
    // Default filter keeps caliban + caliban_* crates at DEBUG and
    // silences noisy lower-level traces (mio, hyper, reqwest, …).
    // Users can override via RUST_LOG env var.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "debug,mio=warn,hyper=warn,hyper_util=warn,reqwest=warn,h2=warn,rustls=warn,tower=warn",
        )
    });
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false);
    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .init();
    tracing::info!("caliban debug logging started — {}", log_path.display());
}

pub(crate) fn build_provider(args: &Args) -> Result<Arc<dyn Provider + Send + Sync>> {
    use ProviderKind::{Anthropic, Google, Ollama, Openai};
    Ok(match args.provider {
        Anthropic => {
            use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
            Arc::new(AnthropicProvider::direct(missing_key(
                "ANTHROPIC_API_KEY",
                DirectConfig::from_env(),
            )?)?)
        }
        Openai => {
            use caliban_provider_openai::{OpenAIProvider, config::DirectConfig};
            Arc::new(OpenAIProvider::direct(missing_key(
                "OPENAI_API_KEY",
                DirectConfig::from_env(),
            )?)?)
        }
        Ollama => {
            use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
            Arc::new(OllamaProvider::direct(
                DirectConfig::from_env().unwrap_or_else(|_| DirectConfig::local()),
            )?)
        }
        Google => {
            use caliban_provider_google::{GoogleProvider, config::AIStudioConfig};
            Arc::new(GoogleProvider::ai_studio(missing_key(
                "GEMINI_API_KEY",
                AIStudioConfig::from_env(),
            )?)?)
        }
    })
}

/// Wrap a provider config-builder failure with a single actionable
/// message instead of an anyhow chain that just re-states the env var
/// name twice. Tells the operator the three ways to supply credentials.
fn missing_key<T, E>(env_var: &'static str, res: std::result::Result<T, E>) -> Result<T> {
    res.map_err(|_| {
        anyhow::anyhow!(
            "{env_var} is not set — export it, configure `apiKeyHelper` in \
             settings.json (ADR 0026), or pick a different `--provider`"
        )
    })
}

pub(crate) fn build_registry(
    args: &Args,
    workspace: WorkspaceRoot,
    todos: caliban_agent_core::SharedTodos,
    plan_mode: caliban_agent_core::SharedPlanMode,
    plugin_skill_roots: &[PathBuf],
) -> ToolRegistry {
    if args.no_tools {
        return ToolRegistry::new();
    }
    let workspace_root = workspace.root().to_path_buf();
    let root = if args.restrict_paths {
        workspace.restricted()
    } else {
        workspace
    };
    let mut r = ToolRegistry::new();
    r.register(Arc::new(ReadTool::new(root.clone())));
    r.register(Arc::new(WriteTool::new(root.clone())));
    r.register(Arc::new(EditTool::new(root.clone())));
    r.register(Arc::new(MultiEditTool::new(root.clone())));
    r.register(Arc::new(NotebookEditTool::new(root.clone())));
    r.register(Arc::new(BashTool::new(root.clone())));
    r.register(Arc::new(GlobTool::new(root.clone())));
    r.register(Arc::new(GrepTool::new(root)));
    r.register(Arc::new(WebFetchTool::new(web_fetch_client())));
    r.register(Arc::new(WebSearchTool::new(web_fetch_client())));
    r.register(Arc::new(BashOutputTool::with_global_registry()));
    r.register(Arc::new(KillShellTool::with_global_registry()));
    r.register(Arc::new(TodoWriteTool::new(todos)));
    r.register(Arc::new(EnterPlanModeTool::new(Arc::clone(&plan_mode))));
    r.register(Arc::new(ExitPlanModeTool::new(plan_mode)));
    // Auto-memory tools — kill switch via env per ADR 0035. The skill body
    // documents how to use the tools; without the skill, the model has no
    // protocol manual, so we gate both together. Skipped in bare mode.
    if !auto_memory_disabled() && !args.bare {
        let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);
        let topic_loader = Arc::new(caliban_memory::TopicLoader::new(cfg.auto_memory_dir));
        r.register(Arc::new(ReadMemoryTopicTool::new(Arc::clone(
            &topic_loader,
        ))));
        r.register(Arc::new(WriteMemoryTopicTool::new(topic_loader)));
    }

    if !args.no_skills && !args.bare {
        let mut roots = caliban_skills::default_roots(&workspace_root);
        roots.extend(plugin_skill_roots.iter().cloned());
        let mut skills = load_skills(&roots);
        // Built-in skills register *before* user-dir scan results win — except
        // that the loader already shadows duplicates, so `register_builtins`
        // is a no-op if the user shipped their own `auto-memory` skill.
        // We hide the built-in entirely when the kill switch is set, matching
        // the tool gating above.
        if !auto_memory_disabled() {
            register_builtins(&mut skills);
        }
        r.register(Arc::new(SkillTool::new(skills)));
    }
    r
}

/// Drive the layered `settings.json` loader (ADR 0026).
///
/// Honors `--bare`, `--settings`, and `--setting-sources`. When the
/// unified file is absent, legacy `permissions.toml`, `mcp.toml`, and
/// `hooks.toml` paths still load via the existing per-feature loaders
/// (handled in their respective wire-up sites below).
pub(crate) fn load_layered_settings(
    args: &Args,
    workspace_root: &std::path::Path,
) -> Result<caliban_settings::LoadOutcome> {
    let mut opts = caliban_settings::LoadOptions::new(workspace_root.to_path_buf());
    opts.bare = args.bare;
    if let Some(csv) = args.setting_sources.as_deref() {
        opts = opts.with_sources_csv(csv).map_err(|e| anyhow::anyhow!(e))?;
    }
    if let Some(overlay) = args.settings_overlay.as_deref() {
        opts = opts
            .with_cli_overlay(overlay)
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    let outcome = caliban_settings::load_settings(&opts).map_err(|e| anyhow::anyhow!(e))?;
    Ok(outcome)
}

/// Returns true if the user has opted out of the auto-memory feature.
/// Matches the loader-side check in `caliban_memory::loader`.
pub(crate) fn auto_memory_disabled() -> bool {
    matches!(
        std::env::var("CALIBAN_DISABLE_AUTO_MEMORY").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "True" | "yes")
    )
}

/// Build the shared `reqwest::Client` used by [`WebFetchTool`].
///
/// Manual redirect handling is required (the tool applies its own same-host
/// policy and surfaces cross-host redirects), so `Policy::none()` is set
/// here. A separate client is intentional — provider transports configure
/// their own clients and have different timeout/keep-alive needs.
pub(crate) fn web_fetch_client() -> reqwest::Client {
    caliban_common::http::no_redirect_client()
}

pub(crate) async fn run_and_render(
    agent: Arc<Agent>,
    messages: Vec<Message>,
    cancel: CancellationToken,
    quiet: bool,
) -> Result<(Vec<Message>, Usage, caliban_agent_core::StopCondition)> {
    use caliban_agent_core::TurnEvent;

    let mut stream = agent.stream_until_done(messages, cancel);
    let mut tool_inputs: HashMap<String, String> = HashMap::new();
    let mut at_column_zero = true;
    let mut final_messages: Vec<Message> = Vec::new();
    let mut total_usage = Usage::default();
    let mut final_stop = caliban_agent_core::StopCondition::EndOfTurn;

    // Honor NO_COLOR (https://no-color.org/) and skip ANSI when stderr
    // is not a TTY. Color is purely decorative here.
    let use_color = {
        use std::io::IsTerminal as _;
        std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
    };
    let dim_on = if use_color { "\x1b[2m" } else { "" };
    let dim_off = if use_color { "\x1b[0m" } else { "" };

    while let Some(event) = stream.next().await {
        match event? {
            TurnEvent::AssistantTextDelta { text, .. } => {
                print!("{text}");
                std::io::stdout().flush().ok();
                at_column_zero = text.ends_with('\n');
            }
            TurnEvent::AssistantThinkingDelta { text, .. } if !quiet => {
                eprint!("{dim_on}{text}{dim_off}");
            }
            TurnEvent::ToolCallStart {
                tool_use_id, name, ..
            } if !quiet => {
                if !at_column_zero {
                    eprintln!();
                }
                tool_inputs.insert(tool_use_id.clone(), String::new());
                eprint!("\u{1f527} {name}(");
            }
            TurnEvent::ToolCallInputDelta {
                tool_use_id,
                partial_json,
                ..
            } => {
                tool_inputs
                    .entry(tool_use_id)
                    .or_default()
                    .push_str(&partial_json);
            }
            TurnEvent::ToolCallEnd {
                tool_use_id,
                is_error,
                content,
                ..
            } if !quiet => {
                let input_str = tool_inputs.remove(&tool_use_id).unwrap_or_default();
                let input_summary = summarize(&input_str, 80);
                let result_summary = summarize_blocks(&content, 80);
                let prefix = if is_error { "(error) " } else { "" };
                eprintln!("{input_summary})");
                eprintln!("   \u{2192} {prefix}{result_summary}");
                at_column_zero = true;
            }
            TurnEvent::RunEnd {
                final_messages: fm,
                total_usage: tu,
                turn_count,
                stopped_for,
            } => {
                if !at_column_zero {
                    println!();
                }
                // F5/F9 follow-up: the TUI + headless drivers surface
                // `stopped_for` for non-`EndOfTurn` variants. The single-
                // prompt CLI driver was missed by the original fix —
                // provider errors and hook-denial were silently swallowed
                // (run exits 0 with empty stdout, no signal). Surface the
                // same one-line description on stderr — even under
                // --quiet — so the run never finishes invisibly.
                if let Some(line) = stopped_for_surface_line(&stopped_for) {
                    eprintln!("{line}");
                }
                // F13: if the model's final assistant message has Thinking
                // blocks but no Text block, the user saw nothing on stdout.
                // Surface a one-line hint on stderr — even under --quiet —
                // so the run isn't silently empty. Common with reasoning
                // models (Qwen3 reasoning, DeepSeek-R1, OpenAI o-series)
                // when an upstream tool error leaves the model with no
                // useful reply to commit to.
                let thinking_only = last_assistant_thinking_only(&fm);
                if thinking_only {
                    let hint = if quiet {
                        "[caliban: model emitted reasoning only — no visible reply (drop --quiet to see reasoning streamed on stderr, or inspect the session JSON)]"
                    } else {
                        "[caliban: model emitted reasoning only — no visible reply]"
                    };
                    eprintln!("{hint}");
                }
                if !quiet {
                    eprintln!(
                        "\n[caliban: {turn_count} turns \u{00b7} {}\u{2191} {}\u{2193} tokens]",
                        tu.input_tokens, tu.output_tokens
                    );
                }
                final_messages = fm;
                total_usage = tu;
                final_stop = stopped_for;
                at_column_zero = true;
            }
            _ => {}
        }
    }

    if !at_column_zero {
        println!();
    }

    Ok((final_messages, total_usage, final_stop))
}

/// Map a [`caliban_agent_core::StopCondition`] to the sysexits-style
/// process exit code per ADR 0025's table. `EndOfTurn` returns `0`;
/// every other variant returns the matching code from the headless
/// driver, so single-prompt mode and `-p` mode exit identically.
pub(crate) fn stop_condition_exit_code(stop: &caliban_agent_core::StopCondition) -> i32 {
    use caliban_agent_core::StopCondition;
    match stop {
        StopCondition::EndOfTurn => 0,
        StopCondition::MaxTurnsReached(_) => 130,
        StopCondition::Cancelled => 124,
        StopCondition::ProviderError(_)
        | StopCondition::HookDenied(_)
        | StopCondition::CompactionFailed(_)
        | StopCondition::Refusal(_)
        | StopCondition::ContentFilter(_)
        | StopCondition::MaxTokensExhausted
        | StopCondition::StreamIdle(_) => 1,
    }
}

/// Map a [`caliban_agent_core::StopCondition`] to a one-line stderr
/// surface for the single-prompt CLI driver. Returns `None` for the
/// natural `EndOfTurn` stop (no surfacing needed). Mirrors the TUI and
/// headless drivers' surfacing of the lmstudio probe's Findings 5 + 9,
/// closing the previously-missed `run_and_render` path.
///
/// Kept separate from `tui::events::stopped_for_surface` (which carries
/// a `level` color hint) so this stays free of tui-specific types and
/// can be unit-tested in isolation.
fn stopped_for_surface_line(stopped_for: &caliban_agent_core::StopCondition) -> Option<String> {
    use caliban_agent_core::StopCondition;
    match stopped_for {
        StopCondition::EndOfTurn => None,
        StopCondition::ProviderError(msg) => Some(format!("[caliban: provider error: {msg}]")),
        StopCondition::HookDenied(msg) => Some(format!("[caliban: hook denied: {msg}]")),
        StopCondition::CompactionFailed(msg) => {
            Some(format!("[caliban: compaction failed: {msg}]"))
        }
        StopCondition::MaxTurnsReached(n) => Some(format!("[caliban: max-turns ({n}) reached]")),
        StopCondition::Cancelled => Some("[caliban: cancelled]".to_string()),
        StopCondition::MaxTokensExhausted => {
            Some("[caliban: max-tokens recovery exhausted]".to_string())
        }
        StopCondition::Refusal(msg) => Some(format!("[caliban: model refusal: {msg}]")),
        StopCondition::ContentFilter(msg) => Some(format!("[caliban: content filter: {msg}]")),
        StopCondition::StreamIdle(d) => Some(format!("[caliban: stream idle for {d:?}]")),
    }
}

/// Return `true` when the last `Assistant` message in `messages` has at
/// least one `Thinking` content block AND zero `Text` content blocks.
/// Used by [`run_and_render`] (lmstudio Finding 13) to surface a hint
/// when a reasoning model's final turn produced reasoning only — the
/// CLI's `--quiet` mode gates thinking-delta streaming on stderr, so
/// otherwise the run looks silently broken.
///
/// Returns `false` if there is no assistant message in the history.
/// Returns `false` if the final assistant message has only `ToolUse`
/// blocks (different scenario — the model chained to a tool and either
/// hit max-turns or stopped before producing text; surfaced separately
/// by the `RunEnd.stopped_for` plumbing).
fn last_assistant_thinking_only(messages: &[Message]) -> bool {
    let Some(last_assistant) = messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, caliban_provider::Role::Assistant))
    else {
        return false;
    };
    let mut has_thinking = false;
    let mut has_text = false;
    for block in &last_assistant.content {
        match block {
            caliban_provider::ContentBlock::Thinking(_) => has_thinking = true,
            caliban_provider::ContentBlock::Text(_) => has_text = true,
            _ => {}
        }
    }
    has_thinking && !has_text
}

/// Source of user prompts for [`run_headless`]. Either a single explicit
/// prompt (resolved from CLI args or plain stdin) or an unparsed NDJSON
/// stream consumed frame-by-frame by [`headless::HeadlessDriver::run_frames`]
/// (lmstudio Finding 10).
enum PromptSource {
    Single(String),
    StreamJson(String),
}

/// Drive the agent loop in headless (`-p` / `--print`) mode and exit with
/// the appropriate process exit code (ADR 0025).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn run_headless(
    args: &Args,
    agent: Arc<Agent>,
    system_prompt: Option<String>,
    todo_snapshot: Vec<caliban_agent_core::Todo>,
    session: Option<PersistedSession>,
    store: Option<SessionStore>,
    todos: caliban_agent_core::SharedTodos,
    plan_mode: caliban_agent_core::SharedPlanMode,
    model: String,
    cancel: CancellationToken,
    hook_event_buffer: Option<headless::HookEventBuffer>,
    plugin_descriptors: Vec<serde_json::Value>,
) -> i32 {
    let output_format = args.output_format.unwrap_or(headless::OutputFormat::Text);

    // Resolve --continue / --resume. They override the in-memory `session`
    // computed by the legacy `--session` flag when both are present.
    let mut session = session;
    if args.continue_latest || args.resume.is_some() {
        let store_for_resume = match store.as_ref() {
            Some(s) => s.clone(),
            None => match SessionStore::default_root() {
                Ok(root) => SessionStore::new(root),
                Err(e) => {
                    eprintln!("[caliban] could not resolve sessions dir: {e}");
                    return 1;
                }
            },
        };
        match headless::session_loader::resolve_session(
            &store_for_resume,
            args.continue_latest,
            args.resume.as_deref(),
        ) {
            Ok(Some(s)) => {
                // Replay todos / plan-mode from the resumed session.
                todos.lock().expect("todos lock").clone_from(&s.todos);
                plan_mode.store(s.plan_mode, std::sync::atomic::Ordering::Relaxed);
                session = Some(s);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[caliban] {e}");
                return headless::exit_code_for(&e);
            }
        }
    }

    // Resolve the prompt source. Three shapes:
    // - An explicit CLI prompt (`--print "x"` / `--prompt` / positional) →
    //   single-frame path; `prompt_source` is `Single(text)`.
    // - No explicit prompt, plain-text stdin → single-frame path with stdin
    //   contents as the prompt.
    // - No explicit prompt, `--input-format stream-json` → multi-frame path;
    //   `prompt_source` is `StreamJson(stdin_input)` and is driven below by
    //   `HeadlessDriver::run_frames` (Finding 10).
    let print_value = args.print.as_deref().filter(|s| !s.is_empty());
    let prompt_source = match (
        print_value,
        args.prompt_flag.as_deref(),
        args.prompt.as_deref(),
    ) {
        (Some(p), _, _) | (_, Some(p), _) | (_, _, Some(p)) => PromptSource::Single(p.to_string()),
        (None, None, None) => {
            let stdin_input = match headless::input::read_stdin_capped(&mut std::io::stdin()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[caliban] {e}");
                    return headless::exit_code_for(&e);
                }
            };
            if matches!(args.input_format, headless::InputFormat::StreamJson) {
                PromptSource::StreamJson(stdin_input)
            } else {
                PromptSource::Single(stdin_input.trim_end_matches('\n').to_string())
            }
        }
    };

    // Reject empty prompts in headless `text` input mode. `-p ""` and
    // empty stdin both land here; running the agent with an empty user
    // message is never useful and produces opaque provider errors.
    // `stream-json` input is allowed to be empty — the multi-frame
    // driver enforces its own `NoUserInput` path with exit 66.
    if let PromptSource::Single(ref p) = prompt_source
        && p.trim().is_empty()
    {
        eprintln!(
            "[caliban] empty prompt — pass a non-empty `--print <TEXT>`, positional arg, or stdin"
        );
        return 64;
    }

    // Permission-prompt-tool: parsed-and-ignored with a warning (ADR 0023
    // Phase C will wire this).
    if let Some(tool) = &args.permission_prompt_tool {
        eprintln!(
            "[caliban] --permission-prompt-tool='{tool}' will route Ask events to the named MCP elicitation tool (ADR 0023 Phase C)"
        );
    }

    // --max-budget-usd is enforced by `caliban-telemetry::pricing` (ADR 0033).
    // No global warning needed — unknown (provider, model) pairs emit a
    // debounced WARN through the budget tracker itself.

    // Optional JSON schema.
    let json_schema = match args.json_schema.as_deref() {
        Some(arg) => match headless::JsonSchema::from_cli_arg(arg) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("[caliban] {e}");
                return headless::exit_code_for(&e);
            }
        },
        None => None,
    };

    // System prompt: install (possibly empty) on a fresh session. The
    // single-frame path also appends the resolved user prompt here; the
    // multi-frame stream-json path defers user-message construction to
    // `HeadlessDriver::run_frames`, which pushes one user message per
    // `User` frame parsed from stdin.
    let mut messages = session
        .as_ref()
        .map(|s| s.messages.clone())
        .unwrap_or_default();
    let has_system = messages
        .first()
        .is_some_and(|m| m.role == caliban_provider::Role::System);
    if !has_system && let Some(ref sp) = system_prompt {
        let with_todos = system_prompt::append_todo_block(sp, &todo_snapshot);
        messages.insert(0, caliban_provider::Message::system_text(with_todos));
    }
    if let PromptSource::Single(ref prompt_text) = prompt_source {
        messages.push(Message::user_text(prompt_text.clone()));
    }

    // Setting source-chain — for now we synthesize a static chain that
    // mirrors what the binary loads. ADR 0026 (`settings.json` precedence)
    // will replace this with a real source list.
    let mut setting_sources = vec!["builtin".to_string()];
    if !args.bare {
        if !args.no_hooks {
            setting_sources.push("hooks.toml".into());
        }
        if !args.no_skills {
            setting_sources.push("skills".into());
        }
        if !args.no_mcp {
            setting_sources.push("mcp.toml".into());
        }
        setting_sources.push("memory".into());
    }

    let cwd = std::env::current_dir().map_or_else(|_| ".".to_string(), |p| p.display().to_string());

    let tools: Vec<String> = {
        let mut v: Vec<String> = agent.tools().names().map(str::to_string).collect();
        v.sort();
        v
    };

    let model_summary = format!("{}/{}", provider_name(args.provider), model);
    let session_id = args
        .session
        .clone()
        .or_else(|| args.resume.clone())
        .unwrap_or_else(|| "ephemeral".into());

    let budget = headless::BudgetTracker::new(args.max_budget_usd);

    let config = headless::HeadlessRunConfig {
        output_format,
        input_format: args.input_format,
        // Translate `--max-turns 0` into "short-circuit and return 130".
        max_turns: if args.print.is_some() || args.output_format.is_some() {
            Some(args.max_turns)
        } else {
            None
        },
        budget: Arc::clone(&budget),
        json_schema,
        include_partial_messages: args.include_partial_messages,
        include_hook_events: args.include_hook_events,
        replay_user_messages: args.replay_user_messages,
        bare_mode: args.bare,
        fallback_model: args.fallback_model.clone(),
        session_id,
        setting_sources,
        tools,
        plugins: plugin_descriptors,
        model_summary,
        cwd,
        hook_buffer: hook_event_buffer,
    };

    let stdout = std::io::stdout().lock();
    let writer = std::io::BufWriter::new(stdout);
    let mut driver = headless::HeadlessDriver::new(writer, config);

    // Fire SessionStart hook explicitly so --include-hook-events sees it.
    {
        let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let session_ctx = caliban_agent_core::SessionCtx {
            session_id: args
                .session
                .as_deref()
                .or(args.resume.as_deref())
                .unwrap_or("ephemeral"),
            cwd: &cwd_now,
            provider: provider_name(args.provider),
            model: &model,
        };
        if let Err(e) = agent.hooks().session_start(&session_ctx).await {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_start hook error (non-fatal)");
        }
        // `driver.run()` below emits the canonical `system/init` frame
        // and then drains the hook buffer, so any frames captured here
        // (e.g. `SessionStart`) are flushed in the correct order
        // without a second `emit_init` call (Finding 8).
    }

    let outcome = match prompt_source {
        PromptSource::Single(_) => driver.run(Arc::clone(&agent), messages, cancel).await,
        PromptSource::StreamJson(stdin_input) => {
            driver
                .run_frames(Arc::clone(&agent), messages, &stdin_input, cancel)
                .await
        }
    };

    // Fire SessionEnd hook (best-effort).
    {
        let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let (i_tok, o_tok) = budget.total_tokens();
        let outcome_ctx = caliban_agent_core::SessionOutcome {
            turn_count: 0,
            input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
        };
        let session_ctx = caliban_agent_core::SessionCtx {
            session_id: args
                .session
                .as_deref()
                .or(args.resume.as_deref())
                .unwrap_or("ephemeral"),
            cwd: &cwd_now,
            provider: provider_name(args.provider),
            model: &model,
        };
        if let Err(e) = agent.hooks().session_end(&session_ctx, &outcome_ctx).await {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_end hook error (non-fatal)");
        }
        let _ = driver.flush_hook_events();
    }

    // Save session back if requested.
    if let (Some(store), Some(mut s)) = (store.as_ref(), session)
        && !args.no_save
    {
        // For headless mode we don't have the agent's `final_messages`
        // (the driver consumed them). Approximate by snapshotting todos
        // and bumping updated_at.
        s.touch();
        s.todos
            .clone_from(&*todos.lock().expect("todos lock poisoned"));
        s.plan_mode = plan_mode.load(std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = store.save(&s) {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_SESSIONS, error = %e, "session save failed");
        }
    }

    match outcome {
        Ok(_) => 0,
        Err(e) => {
            // The driver already emitted the result frame for terminal
            // conditions; for non-terminal errors we surface to stderr.
            let code = headless::exit_code_for(&e);
            if !matches!(
                e,
                headless::HeadlessError::MaxTurnsExceeded(_)
                    | headless::HeadlessError::BudgetExceeded { .. }
                    | headless::HeadlessError::Cancelled
                    | headless::HeadlessError::SchemaValidation(_)
            ) {
                eprintln!("[caliban] {e}");
            }
            code
        }
    }
}

/// Discover plugins early (ADR 0030). Plugins contribute skill roots,
/// hooks config, MCP servers, agents, and output styles, so the manager
/// is constructed before any of those subsystems init. `--bare` and the
/// `--no-plugins` kill switch both produce an empty manager.
pub(crate) fn load_plugin_manager(
    args: &Args,
    workspace_root: &std::path::Path,
) -> caliban_plugins::PluginManager {
    if args.bare || args.no_plugins {
        return caliban_plugins::PluginManager::default();
    }
    let ws_for_plugins = args
        .workspace
        .clone()
        .unwrap_or_else(|| workspace_root.to_path_buf());
    let roots = caliban_plugins::PluginRoots::default_for(&ws_for_plugins);
    let settings = caliban_plugins::PluginSettings::from_env();
    match caliban_plugins::PluginManager::load(&roots, &settings) {
        Ok(mgr) => {
            if !mgr.loaded().is_empty() {
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_PLUGINS,
                    count = mgr.loaded().len(),
                    "loaded plugins",
                );
            }
            for f in mgr.failures() {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_PLUGINS,
                    path = %f.root_dir.display(),
                    error = %f.error,
                    "plugin failed to load",
                );
            }
            mgr
        }
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_PLUGINS, error = %e, "plugin discovery failed; continuing without plugins");
            caliban_plugins::PluginManager::default()
        }
    }
}

/// MCP servers — Phase B: stdio + HTTP + SSE transports (ADR 0023).
/// `--bare` (ADR 0025) suppresses MCP discovery entirely for reproducible CI.
/// Returns `(summaries, server_cfg)` — the latter is retained so the
/// permissions setup downstream can fold `[server.X.permissions]` blocks
/// into the global rule list.
///
/// Reads the MCP server map from the unified `Settings` snapshot (ADR 0026);
/// the legacy `caliban_mcp_client::load_config` loader is reachable through
/// the `caliban-settings` compat shim during the one-release deprecation
/// window and is no longer called directly from the binary.
pub(crate) async fn start_mcp(
    args: &Args,
    settings_snapshot: &caliban_settings::Settings,
    registry: &mut ToolRegistry,
) -> (
    Vec<caliban_mcp_client::ServerSummary>,
    std::collections::BTreeMap<String, caliban_mcp_client::ServerConfig>,
) {
    if args.no_mcp || args.bare {
        return (Vec::new(), std::collections::BTreeMap::new());
    }
    let cfg = settings_snapshot.mcp_config();
    let servers_for_perms = cfg.servers.clone();
    match caliban_mcp_client::McpClientManager::start(&cfg).await {
        Ok(mgr) => {
            mgr.register_all(registry);
            if mgr.enabled_count() > 0 || mgr.skipped_disabled() > 0 || mgr.failed_count() > 0 {
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_MCP,
                    connected = mgr.enabled_count(),
                    failed = mgr.failed_count(),
                    disabled = mgr.skipped_disabled(),
                    "mcp manager started",
                );
            }
            (mgr.summaries().to_vec(), servers_for_perms)
        }
        Err(e) => {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_MCP, error = %e, "mcp manager start failed; continuing without MCP");
            (Vec::new(), servers_for_perms)
        }
    }
}

/// Wire `AgentTool` (the sub-agent primitive) into `registry`.
///
/// The factory closes over a snapshot of the parent registry (which DOES
/// NOT include `AgentTool`, so sub-agents cannot recurse) + the parent's
/// provider + chosen model. Hook inheritance is deferred to v2 — sub-agents
/// currently use `NoopHooks`. The background-handoff spawner asks the
/// per-repo supervisor daemon (auto-spawned if needed) to register a new
/// agent and return its socket (ADR 0037).
pub(crate) fn install_sub_agent(
    args: &Args,
    registry: &mut ToolRegistry,
    provider: &Arc<dyn Provider + Send + Sync>,
    model: &str,
) {
    if args.no_sub_agent || args.no_tools {
        return;
    }
    let snapshot_names: Vec<String> = registry.names().map(str::to_string).collect();
    let mut snapshot = ToolRegistry::new();
    for name in &snapshot_names {
        if let Some(t) = registry.get(name) {
            snapshot.register(Arc::clone(t));
        }
    }
    let provider_for_factory: Arc<dyn Provider + Send + Sync> = Arc::clone(provider);
    let parent_model = model.to_string();
    let parent_max_tokens = args.max_tokens;
    let factory: AgentFactory = Arc::new(move |input: &AgentToolInput| {
        let chosen_model = input.model.clone().unwrap_or_else(|| parent_model.clone());
        let child_registry = match &input.tool_allowlist {
            Some(names) => {
                let mut r = ToolRegistry::new();
                for n in names {
                    if n == "AgentTool" {
                        continue;
                    }
                    if let Some(t) = snapshot.get(n) {
                        r.register(Arc::clone(t));
                    }
                }
                r
            }
            None => snapshot.clone(),
        };
        Agent::builder()
            .provider(Arc::clone(&provider_for_factory))
            .tools(child_registry)
            .model(chosen_model)
            .max_tokens(parent_max_tokens)
            .max_turns(20)
            .build()
            .expect("sub-agent builder")
    });
    // Background-handoff spawner (ADR 0037). When the parent invokes
    // AgentTool with `background: true`, the tool calls this closure;
    // we ask the per-repo supervisor daemon (auto-spawned if needed)
    // to register a new agent and return its socket. Closure-based
    // hooks are dropped at the boundary — the parent's `Hooks` chain
    // can't cross processes; see ADR 0037 ("Hook inheritance").
    let cwd_for_bg = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let repo_for_bg = agents_cli::discover_repo_root(&cwd_for_bg);
    let bg_spawner: caliban_tools_builtin::BackgroundSpawner = {
        let repo = repo_for_bg.clone();
        Arc::new(move |input: &AgentToolInput| {
            // Use a blocking handle to the current runtime so the
            // AgentTool stays sync-callable from its async invoke.
            let rt = tokio::runtime::Handle::current();
            let spec = caliban_supervisor::SpawnSpec {
                label: input.label.clone(),
                frontmatter_path: None,
                initial_prompt: input.prompt.clone(),
                model: input.model.clone(),
                tool_allowlist: input.tool_allowlist.clone(),
                isolation_worktree: matches!(
                    input.isolation,
                    caliban_tools_builtin::IsolationMode::Worktree
                ),
                inherit_hooks: input.inherit_hooks,
            };
            let repo = repo.clone();
            // We can't `await` directly inside a non-async closure;
            // block on a fresh task instead.
            let (id, socket_path) = rt
                .block_on(async move {
                    let client = agents_cli::ensure_daemon_for_repo(&repo).await?;
                    client.spawn(spec).await.map_err(anyhow::Error::from)
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "background spawn failed");
                    (
                        format!("err-{}", uuid::Uuid::new_v4().simple()),
                        std::path::PathBuf::from("/dev/null"),
                    )
                });
            caliban_tools_builtin::BackgroundSpawnResult { id, socket_path }
        })
    };
    registry.register(Arc::new(
        AgentTool::new(factory, None).with_background_spawner(bg_spawner),
    ));
}

/// Project the hooks configuration out of the layered `Settings` snapshot.
/// Empty config when `--no-hooks` is set or `--bare` is in effect.
///
/// Reads from `Settings::hook_config()` (ADR 0026) instead of the legacy
/// `caliban_agent_core::HooksConfig::load` loader; the latter is now
/// `#[deprecated]` and remains reachable only through the
/// `caliban-settings::compat` shim during the one-release back-compat
/// window.
pub(crate) fn load_hooks_config(
    args: &Args,
    settings_snapshot: &caliban_settings::Settings,
) -> caliban_agent_core::HooksConfig {
    if args.no_hooks || args.bare {
        return caliban_agent_core::HooksConfig::default();
    }
    settings_snapshot.hook_config()
}

/// Outcome of [`build_permissions`]: the `Hooks` layer (or `None` when
/// `--no-permissions` is set), the `Ask`-bridge receiver for TUI mode,
/// and the auto-mode classifier (cloned later into the TUI driver).
pub(crate) struct PermissionsSetup {
    pub permissions_hook: Option<Arc<dyn caliban_agent_core::Hooks + Send + Sync>>,
    pub tui_ask_rx: Option<tokio::sync::mpsc::UnboundedReceiver<tui::ask::AskRequest>>,
    pub auto_mode_classifier: Option<Arc<caliban_agent_core::AutoModeClassifier>>,
}

/// Build the permissions chain (rules → `ModeFilter` → `PermissionsHook`).
///
/// Layers CLI flags (`--allow` / `--deny` / `--ask`) on top of the rules
/// projected from the layered `Settings` snapshot (`settings.json` plus
/// the legacy `permissions.toml` compat fallback already folded in by
/// `caliban-settings`), then appends the built-in `default_rules` tail
/// and folds per-server `[server.X.permissions]` blocks.
///
/// Returns `PermissionsSetup::default`-equivalent (all-`None`) when
/// `--no-permissions` is set.
pub(crate) fn build_permissions(
    args: &Args,
    settings_snapshot: &caliban_settings::Settings,
    mcp_server_cfg: &std::collections::BTreeMap<String, caliban_mcp_client::ServerConfig>,
    provider: &Arc<dyn Provider + Send + Sync>,
    model: &str,
    permission_mode: &caliban_agent_core::SharedPermissionMode,
    tui_mode_active: bool,
) -> PermissionsSetup {
    use caliban_agent_core::{
        Action, AutoModeClassifier, AutoModeConfig, DEFAULTS_TOKEN, ModeFilter,
        NonInteractiveAskHandler, NoopHooks, PermissionsHook, Rule, default_rules,
    };

    if args.no_permissions {
        return PermissionsSetup {
            permissions_hook: None,
            tui_ask_rx: None,
            auto_mode_classifier: None,
        };
    }
    let mut cli_rules: Vec<Rule> = Vec::new();
    for p in &args.allow {
        cli_rules.push(Rule {
            tool: p.clone(),
            action: Action::Allow,
            comment: None,
        });
    }
    for p in &args.deny {
        cli_rules.push(Rule {
            tool: p.clone(),
            action: Action::Deny,
            comment: None,
        });
    }
    for p in &args.ask {
        cli_rules.push(Rule {
            tool: p.clone(),
            action: Action::Ask,
            comment: None,
        });
    }
    // Layer Settings permission rules (which already incorporate the
    // legacy permissions.toml via the caliban-settings compat shim) at
    // lower priority than CLI flags. The built-in default-rules tail
    // closes the chain (catch-all `*` Ask).
    let mut global_rules = cli_rules;
    for r in settings_snapshot.permission_rules() {
        global_rules.push(r);
    }
    global_rules.extend(default_rules());
    // Phase B: fold per-server `[server.X.permissions]` blocks into the
    // global rule list at the documented priority slot
    // (global deny → server deny/ask/allow → global ask/allow → default).
    let rules = caliban_mcp_client::merge_with_global(global_rules, mcp_server_cfg);
    // In interactive (TUI) mode, route Ask through the modal bridge. In
    // headless/single-prompt mode, fall back to the non-interactive handler.
    let (ask, ask_rx): (Arc<dyn caliban_agent_core::AskHandler>, _) = if tui_mode_active {
        let (handler, rx) = tui::TuiAskHandler::pair();
        (Arc::new(handler), Some(rx))
    } else {
        (
            Arc::new(NonInteractiveAskHandler {
                auto_allow: args.auto_allow,
            }),
            None,
        )
    };
    let inner: Arc<dyn caliban_agent_core::Hooks> =
        Arc::new(PermissionsHook::new(rules, ask, Arc::new(NoopHooks)));

    // Build the auto-mode classifier. The provider is the same one wired
    // for the agent; when it's a router, FastClassifier requests route
    // to whichever model the operator configured for that purpose.
    let auto_cfg = AutoModeConfig {
        environment: vec![DEFAULTS_TOKEN.into()],
        allow: vec![DEFAULTS_TOKEN.into()],
        soft_deny: vec![DEFAULTS_TOKEN.into()],
        hard_deny: vec![DEFAULTS_TOKEN.into()],
        disabled: args.disable_auto_mode,
    };
    let classifier = Arc::new(AutoModeClassifier::new(
        Arc::clone(provider),
        model,
        auto_cfg,
    ));

    let filter: Arc<dyn caliban_agent_core::Hooks + Send + Sync> = Arc::new(ModeFilter::new(
        permission_mode.clone(),
        inner,
        Some(Arc::clone(&classifier)),
        args.allow_dangerously_skip_permissions,
    ));
    PermissionsSetup {
        permissions_hook: Some(filter),
        tui_ask_rx: ask_rx,
        auto_mode_classifier: Some(classifier),
    }
}

/// Fire the `session_start` (or `session_end`) hook with the standard
/// session context. Errors are logged-and-swallowed (best-effort).
pub(crate) async fn fire_session_start(args: &Args, agent: &Arc<Agent>, model: &str) {
    let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let session_id = args.session.clone().unwrap_or_else(|| "ephemeral".into());
    let session_ctx = caliban_agent_core::SessionCtx {
        session_id: &session_id,
        cwd: &cwd_now,
        provider: provider_name(args.provider),
        model,
    };
    if let Err(e) = agent.hooks().session_start(&session_ctx).await {
        tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_start hook error (non-fatal)");
    }
}

/// Fire the `session_end` hook with the standard session context.
/// Errors are logged-and-swallowed (best-effort).
pub(crate) async fn fire_session_end(
    args: &Args,
    agent: &Arc<Agent>,
    model: &str,
    total_usage: &Usage,
) {
    let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let session_id = args.session.clone().unwrap_or_else(|| "ephemeral".into());
    let session_ctx = caliban_agent_core::SessionCtx {
        session_id: &session_id,
        cwd: &cwd_now,
        provider: provider_name(args.provider),
        model,
    };
    let outcome = caliban_agent_core::SessionOutcome {
        turn_count: 0, // not tracked at this layer; populated from final_messages by callers.
        input_tokens: total_usage.input_tokens,
        output_tokens: total_usage.output_tokens,
    };
    if let Err(e) = agent.hooks().session_end(&session_ctx, &outcome).await {
        tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_end hook error (non-fatal)");
    }
}

/// Drive the single-prompt path (no `-p`, no TUI): assembles the initial
/// message list, registers the Ctrl-C handler, runs the agent loop via
/// [`run_and_render`], fires the `session_end` hook, and optionally
/// persists the session back to disk.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_single_prompt(
    args: &Args,
    agent: Arc<Agent>,
    system_prompt: Option<String>,
    todo_snapshot: Vec<caliban_agent_core::Todo>,
    mut session: Option<PersistedSession>,
    store: Option<SessionStore>,
    todos: caliban_agent_core::SharedTodos,
    plan_mode: caliban_agent_core::SharedPlanMode,
    model: String,
) -> Result<()> {
    // Honor `--continue` / `--resume <NAME>` in single-prompt mode with
    // the same semantics the headless driver uses (`ResumeNotFound` →
    // exit 66, `NoSessionsToContinue` → exit 66). Without this both
    // flags silently no-op when `--session` is absent.
    if args.continue_latest || args.resume.is_some() {
        let store_for_resume = match store.as_ref() {
            Some(s) => s.clone(),
            None => SessionStore::new(SessionStore::default_root()?),
        };
        match headless::session_loader::resolve_session(
            &store_for_resume,
            args.continue_latest,
            args.resume.as_deref(),
        ) {
            Ok(Some(s)) => {
                todos.lock().expect("todos lock").clone_from(&s.todos);
                plan_mode.store(s.plan_mode, std::sync::atomic::Ordering::Relaxed);
                session = Some(s);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[caliban] {e}");
                std::process::exit(headless::exit_code_for(&e));
            }
        }
    }

    let cancel = CancellationToken::new();
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("\n[caliban: cancelling\u{2026}]");
            cancel.cancel();
            let _ = tokio::signal::ctrl_c().await;
            std::process::exit(130);
        });
    }

    let prompt = crate::args::read_prompt(args)?;

    // Build initial messages: prior session history (or system prompt) + new user prompt.
    let mut messages = session
        .as_ref()
        .map(|s| s.messages.clone())
        .unwrap_or_default();

    // Ephemeral mode (no session): prepend system prompt (with todos) if not
    // already present.
    let has_system = messages
        .first()
        .is_some_and(|m| m.role == caliban_provider::Role::System);
    if !has_system && let Some(ref sp) = system_prompt {
        let with_todos = system_prompt::append_todo_block(sp, &todo_snapshot);
        messages.insert(0, caliban_provider::Message::system_text(with_todos));
    }

    messages.push(Message::user_text(prompt));

    let (final_messages, total_usage, stop_condition) =
        run_and_render(Arc::clone(&agent), messages, cancel, args.quiet).await?;

    fire_session_end(args, &agent, &model, &total_usage).await;

    // Save session back if requested. The session is persisted before we
    // exit on a non-zero stop code — operators can resume the run that
    // failed instead of losing progress.
    if let (Some(store), Some(ref mut s)) = (store.as_ref(), session.as_mut())
        && !args.no_save
    {
        s.merge_run(final_messages, total_usage);
        s.todos
            .clone_from(&*todos.lock().expect("todos lock poisoned"));
        s.plan_mode = plan_mode.load(std::sync::atomic::Ordering::Relaxed);
        store.save(s)?;
        if !args.quiet {
            let cache_extra = match (
                s.total_usage.cache_read_input_tokens.unwrap_or(0),
                s.total_usage.cache_creation_input_tokens.unwrap_or(0),
            ) {
                (0, 0) => String::new(),
                (r, 0) => format!(", {r} cached"),
                (0, c) => format!(", {c} cache write"),
                (r, c) => format!(", {r} cached, {c} write"),
            };
            eprintln!(
                "[caliban: saved session '{}' ({} turns, {} tokens{})]",
                s.name,
                s.turn_count(),
                s.total_usage.input_tokens + s.total_usage.output_tokens,
                cache_extra,
            );
        }
    }

    // Map the non-`EndOfTurn` stop to the matching sysexits code so
    // single-prompt mode is exit-code-compatible with `-p` (ADR 0025).
    let code = stop_condition_exit_code(&stop_condition);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Build the agent: wire the provider + registry, install the output-
/// style post-processor when the `Learning` style is active, compose the
/// hook chain (`HeadlessHookSink` + `PermissionsHook`), and apply the
/// `--parallel-tool-limit` / `--temperature` knobs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_agent(
    args: &Args,
    provider: Arc<dyn Provider + Send + Sync>,
    registry: ToolRegistry,
    model: &str,
    plan_mode: &caliban_agent_core::SharedPlanMode,
    permissions_hook: Option<Arc<dyn caliban_agent_core::Hooks + Send + Sync>>,
    hook_event_buffer: Option<&headless::HookEventBuffer>,
) -> Result<Arc<Agent>> {
    let mut builder = Agent::builder()
        .provider(provider)
        .tools(registry)
        .model(model.to_string())
        .max_tokens(args.max_tokens)
        .max_turns(args.max_turns)
        .prompt_cache(!args.no_prompt_cache)
        .parallel_tools(!args.no_parallel_tools)
        .plan_mode(Arc::clone(plan_mode));
    // Install the output-style post-processor. Today only the `Learning`
    // style mutates assistant text; everything else uses the identity
    // post-processor (which the agent core already defaults to).
    {
        let workspace_root_for_style = args.workspace.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        let style_reg =
            caliban_output_styles::OutputStylesRegistry::load(&workspace_root_for_style);
        let requested = caliban_output_styles::requested_from_env();
        // v2: enabled_plugins is empty until ADR 0030 plugin system ships.
        let active = style_reg.select(&requested, &[]);
        if let Some(s) = active.as_ref()
            && s.name == "learning"
        {
            let pp: Arc<dyn caliban_agent_core::AssistantPostProcessor> =
                Arc::new(caliban_output_styles::LearningPostProcessor::new());
            builder = builder.post_processor(pp);
        }
    }
    // Compose hooks. When `--include-hook-events` is set, attach a
    // `HeadlessHookSink` at the outermost position so every event becomes
    // an observable frame (ADR 0025). Headless-only — TUI mode ignores it.
    {
        let mut layers: Vec<Arc<dyn caliban_agent_core::Hooks>> = Vec::new();
        if let Some(buf) = hook_event_buffer {
            layers.push(Arc::new(headless::HeadlessHookSink::new(Arc::clone(buf))));
        }
        if let Some(p) = permissions_hook {
            // PermissionsHook is `Send + Sync` but CompositeHooks accepts
            // `Arc<dyn Hooks>` (the trait bound is `Send + Sync` on the
            // supertrait), so coerce.
            layers.push(p as Arc<dyn caliban_agent_core::Hooks>);
        }
        if !layers.is_empty() {
            let composite: Arc<dyn caliban_agent_core::Hooks + Send + Sync> =
                Arc::new(caliban_agent_core::CompositeHooks::new(layers));
            builder = builder.hooks(composite);
        }
    }
    if let Some(limit) = args.parallel_tool_limit {
        builder = builder.parallel_tool_limit(limit);
    }
    if let Some(t) = args.temperature {
        builder = builder.temperature(t);
    }
    Ok(Arc::new(builder.build()?))
}

/// Resolve the session store and load (or create) the persisted session.
/// Seeds the shared todos handle + plan-mode flag from the persisted
/// snapshot if any.
pub(crate) fn resolve_session(
    args: &Args,
    model: &str,
    todos: &caliban_agent_core::SharedTodos,
    plan_mode: &caliban_agent_core::SharedPlanMode,
) -> Result<(Option<SessionStore>, Option<PersistedSession>)> {
    let store = match (&args.sessions_dir, &args.session) {
        (_, None) => None,
        (Some(d), Some(_)) => Some(SessionStore::new(d.clone())),
        (None, Some(_)) => Some(SessionStore::new(SessionStore::default_root()?)),
    };
    let session = if let (Some(store), Some(name)) = (&store, &args.session) {
        Some(match store.load(name)? {
            Some(existing) => existing,
            None => PersistedSession::new(
                name.clone(),
                provider_name(args.provider),
                model.to_string(),
            ),
        })
    } else {
        None
    };
    if let Some(sess) = session.as_ref() {
        todos
            .lock()
            .expect("todos lock poisoned")
            .clone_from(&sess.todos);
        plan_mode.store(sess.plan_mode, std::sync::atomic::Ordering::Relaxed);
    }
    Ok((store, session))
}

/// Resolve the effective system prompt for this run. Handles the
/// `--system` / `--system-file` / `--no-system` overrides, then (when
/// the default prompt is in effect) layers the active output-style
/// block and the memory-tier prefix on top.
pub(crate) async fn resolve_system_prompt(
    args: &Args,
    agent: &Arc<Agent>,
    cwd_for_prompt: &std::path::Path,
    settings_snapshot: &caliban_settings::Settings,
) -> Result<Option<String>> {
    let tool_names: Vec<&str> = agent.tools().names().collect();
    let default_prompt_in_effect =
        args.system.is_none() && args.system_file.is_none() && !args.no_system;
    let system_prompt = system_prompt::resolve(
        args.system.as_deref(),
        args.system_file.as_deref(),
        args.no_system,
        cwd_for_prompt,
        &tool_names,
        args.no_tools,
    )?;

    // Load memory tiers and splice into the default system prompt, then
    // wrap with the active output-style block (after memory, before the
    // base body). The operator's --system / --system-file / --no-system
    // always wins — those paths intentionally skip both memory and output
    // styles.
    let Some(body) = system_prompt else {
        return Ok(None);
    };
    if !default_prompt_in_effect {
        return Ok(Some(body));
    }

    let workspace_root = args
        .workspace
        .clone()
        .unwrap_or_else(|| cwd_for_prompt.to_path_buf());

    // Resolve the active output style. Selection precedence:
    //   1. `force_for_plugin` on a plugin-supplied style (v2 — inert
    //      until ADR 0030 plugin system lands).
    //   2. `CALIBAN_OUTPUT_STYLE` env var (settings.json key with
    //      ADR 0026).
    //   3. Built-in `default` (no-op).
    let style_registry = caliban_output_styles::OutputStylesRegistry::load(&workspace_root);
    let requested = caliban_output_styles::requested_from_env();
    // v2: enabled_plugins is empty until ADR 0030 ships the plugin system.
    let enabled_plugins: Vec<String> = Vec::new();
    let active_style = style_registry.select(&requested, &enabled_plugins);
    let style_prefix = caliban_output_styles::OutputStylePrefix::new(active_style.clone());

    // When the active style requests `keep_coding_instructions: false`,
    // replace the default coding-assistant body with the style body so
    // the prompt does not double up on guidance. The style body is
    // already wrapped in `<output-style>` tags by `splice_into`, so we
    // just feed an empty `base` to the splice.
    let base_body = if style_prefix.drops_coding_instructions() {
        String::new()
    } else {
        body
    };

    // Layering: memory tiers first (highest cache-key precedence), then
    // the output-style block, then the base body. We construct from the
    // inside out — wrap the base body with the style prefix, then wrap
    // that with the memory prefix.
    let with_style = style_prefix.splice_into(&base_body);
    // --bare (ADR 0025) skips auto-memory load entirely.
    let final_prompt = if args.bare {
        with_style
    } else {
        let cfg = apply_memory_settings(
            caliban_memory::MemoryConfig::from_env(&workspace_root),
            settings_snapshot,
        );
        match caliban_memory::load(&cfg).await {
            Ok(prefix) => prefix.splice_into(&with_style),
            Err(e) => {
                tracing::warn!(target: caliban_common::tracing_targets::TARGET_MEMORY, error = %e, "memory load failed; using default prompt without memory");
                with_style
            }
        }
    };
    Ok(Some(final_prompt))
}

/// Overlay `settings.memory.cap_tokens_*` (when present) onto a `MemoryConfig`
/// built from env defaults. Settings values take precedence over env vars when
/// both are set; missing settings keys leave the env-derived value in place.
///
/// Honored keys (all integer, non-negative):
/// - `memory.cap_tokens_combined` → `max_tokens`
/// - `memory.cap_tokens_auto` → per-scope auto-tier cap
/// - `memory.cap_tokens_claude_md` → per-scope CLAUDE.md-tier cap (global + project)
fn apply_memory_settings(
    mut cfg: caliban_memory::MemoryConfig,
    settings_snapshot: &caliban_settings::Settings,
) -> caliban_memory::MemoryConfig {
    let Some(memory) = settings_snapshot.memory.as_ref() else {
        return cfg;
    };
    let read_usize = |key: &str| {
        memory
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    if let Some(n) = read_usize("cap_tokens_combined") {
        cfg.max_tokens = n;
    }
    if let Some(n) = read_usize("cap_tokens_auto") {
        cfg = cfg.with_cap_tokens_auto(n);
    }
    if let Some(n) = read_usize("cap_tokens_claude_md") {
        cfg = cfg.with_cap_tokens_claude_md(n);
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::{last_assistant_thinking_only, stopped_for_surface_line};
    use caliban_agent_core::StopCondition;
    use caliban_provider::{ContentBlock, Message, Role, TextBlock, ThinkingBlock};

    fn thinking(text: &str) -> ContentBlock {
        ContentBlock::Thinking(ThinkingBlock {
            thinking: text.into(),
            signature: None,
        })
    }

    fn text(text: &str) -> ContentBlock {
        ContentBlock::Text(TextBlock {
            text: text.into(),
            cache_control: None,
        })
    }

    fn assistant(blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: Role::Assistant,
            content: blocks,
        }
    }

    fn user_text(s: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![text(s)],
        }
    }

    #[test]
    fn detects_thinking_only_final_turn() {
        // F13 reproduction: a final assistant turn carrying only a Thinking
        // block (the symptom seen when a reasoning model has no useful
        // reply after a tool error).
        let messages = vec![
            user_text("hi"),
            assistant(vec![thinking("I have nothing to say.")]),
        ];
        assert!(last_assistant_thinking_only(&messages));
    }

    #[test]
    fn text_block_disables_hint() {
        // Final assistant has both Thinking and Text → user already saw a
        // reply on stdout; no hint.
        let messages = vec![
            user_text("hi"),
            assistant(vec![thinking("reasoning..."), text("the answer")]),
        ];
        assert!(!last_assistant_thinking_only(&messages));
    }

    #[test]
    fn text_only_disables_hint() {
        let messages = vec![user_text("hi"), assistant(vec![text("answer")])];
        assert!(!last_assistant_thinking_only(&messages));
    }

    #[test]
    fn empty_history_disables_hint() {
        // No assistant message → no hint (typical of immediate-failure runs
        // surfaced via stopped_for separately).
        assert!(!last_assistant_thinking_only(&[]));
    }

    #[test]
    fn only_inspects_last_assistant_message() {
        // Earlier assistant turn was thinking-only (intermediate reasoning
        // before a tool call); final assistant turn produced text. No hint.
        let messages = vec![
            user_text("hi"),
            assistant(vec![thinking("step one")]),
            user_text("more"),
            assistant(vec![text("final answer")]),
        ];
        assert!(!last_assistant_thinking_only(&messages));
    }

    #[test]
    fn ignores_intervening_user_messages_when_finding_last_assistant() {
        // Final message is a tool_result user message; the prior assistant
        // turn (thinking-only) is what matters.
        let messages = vec![
            user_text("hi"),
            assistant(vec![thinking("thinking...")]),
            user_text("(tool_result placeholder)"),
        ];
        assert!(last_assistant_thinking_only(&messages));
    }

    #[test]
    fn no_thinking_block_disables_hint() {
        // Assistant message with no content at all (edge case after a
        // provider error before any deltas land) → no hint, the
        // stopped_for surface handles that separately.
        let messages = vec![user_text("hi"), assistant(vec![])];
        assert!(!last_assistant_thinking_only(&messages));
    }

    // ---- F5/F9 follow-up: stopped_for surfacing in single-prompt CLI ----

    #[test]
    fn end_of_turn_does_not_surface() {
        assert!(stopped_for_surface_line(&StopCondition::EndOfTurn).is_none());
    }

    #[test]
    fn provider_error_surfaces_with_message() {
        let line = stopped_for_surface_line(&StopCondition::ProviderError(
            "context length exceeded".into(),
        ))
        .expect("provider error must surface");
        assert!(line.contains("provider error"));
        assert!(line.contains("context length exceeded"));
        assert!(
            line.starts_with("[caliban:") && line.ends_with(']'),
            "must use the [caliban: …] chrome; got {line}"
        );
    }

    #[test]
    fn hook_denied_surfaces_with_message() {
        let line = stopped_for_surface_line(&StopCondition::HookDenied("policy x".into()))
            .expect("hook-denied must surface");
        assert!(line.contains("hook denied"));
        assert!(line.contains("policy x"));
    }

    #[test]
    fn compaction_failed_surfaces_with_message() {
        let line =
            stopped_for_surface_line(&StopCondition::CompactionFailed("summarizer 503".into()))
                .expect("compaction failure must surface");
        assert!(line.contains("compaction failed"));
        assert!(line.contains("summarizer 503"));
    }

    #[test]
    fn max_turns_surfaces_with_count() {
        let line = stopped_for_surface_line(&StopCondition::MaxTurnsReached(50))
            .expect("max-turns must surface");
        assert!(line.contains("max-turns"));
        assert!(line.contains("50"));
    }

    #[test]
    fn cancelled_surfaces() {
        let line =
            stopped_for_surface_line(&StopCondition::Cancelled).expect("cancellation must surface");
        assert!(line.contains("cancelled"));
    }
}
