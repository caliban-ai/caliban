//! caliban — agent harness binary.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::multiple_crate_versions)]

mod headless;
mod plugin_cli;
mod router;
mod system_prompt;
mod tui;

use std::collections::HashMap;
use std::io::Write as _;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use caliban_agent_core::{Agent, Message, ToolRegistry};
use caliban_provider::{ContentBlock, Provider, Usage};
use caliban_sessions::{PersistedSession, SessionStore};
use caliban_skills::{SkillTool, load_skills, register_builtins};
use caliban_tools_builtin::{
    AgentFactory, AgentTool, AgentToolInput, BashTool, EditTool, EnterPlanModeTool,
    ExitPlanModeTool, GlobTool, GrepTool, ReadMemoryTopicTool, ReadTool, TodoWriteTool,
    WebFetchTool, WorkspaceRoot, WriteMemoryTopicTool, WriteTool,
};
use clap::{Parser, ValueEnum};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ProviderKind {
    Anthropic,
    Openai,
    Ollama,
    Google,
}

pub(crate) fn default_model_for(p: ProviderKind) -> &'static str {
    match p {
        ProviderKind::Anthropic => "claude-3-5-sonnet",
        ProviderKind::Openai => "gpt-4o",
        ProviderKind::Ollama => "llama3.1",
        ProviderKind::Google => "gemini-2.0-flash",
    }
}

fn provider_name(p: ProviderKind) -> &'static str {
    match p {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Openai => "openai",
        ProviderKind::Ollama => "ollama",
        ProviderKind::Google => "google",
    }
}

#[derive(Debug, Clone, Parser)]
#[command(name = "caliban", version, about = "caliban agent harness")]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Args {
    /// User prompt. Use "-" to read from stdin.
    #[arg(value_name = "PROMPT")]
    pub(crate) prompt: Option<String>,

    /// Alternative way to specify the prompt
    #[arg(long = "prompt", value_name = "PROMPT")]
    pub(crate) prompt_flag: Option<String>,

    /// Headless / print mode (ADR 0025). When set, drives the agent
    /// non-interactively and emits text / JSON / NDJSON output. Accepts an
    /// optional prompt argument; otherwise reads from `--prompt`, the
    /// positional `PROMPT`, or stdin (capped at 10 MiB).
    #[arg(short = 'p', long = "print", value_name = "PROMPT", num_args = 0..=1, default_missing_value = "")]
    pub(crate) print: Option<String>,

    /// Stream-output format (headless mode only).
    #[arg(long = "output-format", value_enum, value_name = "FMT")]
    pub(crate) output_format: Option<headless::OutputFormat>,

    /// Stdin format (headless mode only).
    #[arg(
        long = "input-format",
        value_enum,
        value_name = "FMT",
        default_value = "text"
    )]
    pub(crate) input_format: headless::InputFormat,

    /// Maximum cumulative cost in USD before the run aborts (exit 137).
    /// Placeholder enforcement until ADR 0033 wires real cost.
    #[arg(long = "max-budget-usd", value_name = "USD")]
    pub(crate) max_budget_usd: Option<f64>,

    /// Skip hooks/skills/plugins/MCP/auto-memory/CLAUDE.md discovery
    /// (deterministic CI mode; ADR 0025).
    #[arg(long = "bare")]
    pub(crate) bare: bool,

    /// Force structured final output matching the given JSON Schema. Value
    /// can be inline JSON or a path to a `.json` file.
    #[arg(long = "json-schema", value_name = "FILE_OR_JSON")]
    pub(crate) json_schema: Option<String>,

    /// Emit assistant text deltas as separate `text` frames in
    /// stream-json mode (default: aggregate into one `message` frame).
    #[arg(long = "include-partial-messages")]
    pub(crate) include_partial_messages: bool,

    /// Emit a `hook_event` frame per fired hook event in stream-json mode.
    #[arg(long = "include-hook-events")]
    pub(crate) include_hook_events: bool,

    /// Echo each user prompt as a `user` frame in stream-json mode.
    #[arg(long = "replay-user-messages")]
    pub(crate) replay_user_messages: bool,

    /// Resume the most recently updated session.
    #[arg(short = 'c', long = "continue")]
    pub(crate) continue_latest: bool,

    /// Resume a named session.
    #[arg(short = 'r', long = "resume", value_name = "NAME")]
    pub(crate) resume: Option<String>,

    /// Fallback model to use when the primary model errors. Router v2
    /// wires this end-to-end; v1 records and surfaces it in init frames.
    #[arg(long = "fallback-model", value_name = "MODEL")]
    pub(crate) fallback_model: Option<String>,

    /// Route permission `Ask` events to the named MCP tool. Parsed for
    /// forward-compat; MCP elicitation lands with Phase C (ADR 0023).
    #[arg(long = "permission-prompt-tool", value_name = "MCP_TOOL")]
    pub(crate) permission_prompt_tool: Option<String>,

    /// Which provider to use
    #[arg(long, value_enum, default_value_t = ProviderKind::Anthropic)]
    pub(crate) provider: ProviderKind,

    /// Model name (defaults per provider)
    #[arg(long)]
    pub(crate) model: Option<String>,

    /// Per-turn output token limit
    #[arg(long, default_value_t = 2048)]
    pub(crate) max_tokens: u32,

    /// Maximum agent loop iterations
    #[arg(long, default_value_t = 50)]
    pub(crate) max_turns: u32,

    /// Sampling temperature
    #[arg(long)]
    pub(crate) temperature: Option<f32>,

    /// Workspace root for file/shell tools
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Disable all tools (chat-only mode)
    #[arg(long)]
    pub(crate) no_tools: bool,

    /// Reject tool paths outside the workspace root
    #[arg(long)]
    pub(crate) restrict_paths: bool,

    /// Suppress tool-execution announcements
    #[arg(long)]
    pub(crate) quiet: bool,

    /// Load or create a named session; persists to ~/.local/share/caliban/sessions/<NAME>.json.
    #[arg(long, value_name = "NAME")]
    pub(crate) session: Option<String>,

    /// Don't save the session back to disk after the run.
    #[arg(long)]
    pub(crate) no_save: bool,

    /// Override the sessions directory.
    #[arg(long, value_name = "DIR")]
    pub(crate) sessions_dir: Option<PathBuf>,

    /// Override system prompt with the given text.
    #[arg(long, value_name = "STRING", conflicts_with_all = ["system_file", "no_system"])]
    pub(crate) system: Option<String>,

    /// Override system prompt with the contents of a file.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["system", "no_system"])]
    pub(crate) system_file: Option<PathBuf>,

    /// Run with no system prompt (disables the default).
    #[arg(long, conflicts_with_all = ["system", "system_file"])]
    pub(crate) no_system: bool,

    /// Append-log events + draws to ~/.cache/caliban/debug.log
    #[arg(long)]
    pub(crate) debug: bool,

    /// Maximum size of a single `@`-attachment in bytes (default 256 KB).
    #[arg(long, default_value_t = 262_144, env = "CALIBAN_MAX_ATTACH_BYTES")]
    pub(crate) max_attach_bytes: u64,

    /// Aggregate size cap across all `@`-attachments in one message (default 1 MB).
    #[arg(long, default_value_t = 1_048_576, env = "CALIBAN_ATTACH_BUDGET_BYTES")]
    pub(crate) attach_budget_bytes: u64,

    /// Disable Anthropic-style prompt caching (default: enabled).
    #[arg(long, env = "CALIBAN_NO_PROMPT_CACHE")]
    pub(crate) no_prompt_cache: bool,

    /// Disable parallel tool execution (run `tool_use` blocks serially).
    #[arg(long, env = "CALIBAN_NO_PARALLEL_TOOLS")]
    pub(crate) no_parallel_tools: bool,

    /// Max concurrent tool invocations per turn. Defaults to CPU cores - 1 (min 1).
    #[arg(long, value_name = "N", env = "CALIBAN_PARALLEL_TOOL_LIMIT")]
    pub(crate) parallel_tool_limit: Option<NonZeroUsize>,

    /// Disable the Skill tool (no skill discovery at startup).
    #[arg(long, env = "CALIBAN_NO_SKILLS")]
    pub(crate) no_skills: bool,

    /// Disable MCP server discovery (skip loading `mcp.toml`).
    #[arg(long, env = "CALIBAN_NO_MCP")]
    pub(crate) no_mcp: bool,

    /// Disable plugin discovery (ADR 0030). Skips scanning all plugin roots
    /// (project, user, managed) and treats `CALIBAN_ENABLED_PLUGINS` as empty.
    #[arg(long, env = "CALIBAN_NO_PLUGINS")]
    pub(crate) no_plugins: bool,

    /// Disable permission gating entirely (all tool calls allowed).
    #[arg(long, env = "CALIBAN_NO_PERMISSIONS", conflicts_with_all = ["allow", "deny", "ask", "auto_allow"])]
    pub(crate) no_permissions: bool,

    /// Add an Allow rule at top priority (repeatable). Pattern is `Tool` or `Tool:first-arg-glob`.
    #[arg(long = "allow", value_name = "PAT")]
    pub(crate) allow: Vec<String>,

    /// Add a Deny rule at top priority (repeatable).
    #[arg(long = "deny", value_name = "PAT")]
    pub(crate) deny: Vec<String>,

    /// Add an Ask rule at top priority (repeatable).
    #[arg(long = "ask", value_name = "PAT")]
    pub(crate) ask: Vec<String>,

    /// DANGEROUS: allow the model to run any Ask-rule tool without prompting in non-interactive mode.
    #[arg(long, env = "CALIBAN_AUTO_ALLOW")]
    pub(crate) auto_allow: bool,

    /// Disable the built-in `AgentTool` (the sub-agent primitive).
    #[arg(long, env = "CALIBAN_NO_SUB_AGENT")]
    pub(crate) no_sub_agent: bool,

    /// Bypass every external hook handler (debugging escape hatch). Mirrors
    /// the `disable_all_hooks` field in `hooks.toml` but applies one-off.
    /// In-process hooks (`PermissionsHook`, audit) still run.
    #[arg(long, env = "CALIBAN_NO_HOOKS")]
    pub(crate) no_hooks: bool,

    /// Explicit path to `caliban.toml` (overrides walk-up discovery).
    /// When the file exists and declares `[router]`, the binary wires a
    /// model router instead of the single-provider fallback (ADR 0038).
    #[arg(long = "config", value_name = "PATH", env = "CALIBAN_ROUTER_CONFIG")]
    pub(crate) config_path: Option<PathBuf>,

    /// Diagnostic / management subcommands.
    #[command(subcommand)]
    pub(crate) command: Option<CalibanCommand>,
}

/// `caliban router debug ...` subcommand family.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum CalibanCommand {
    /// Router diagnostics (resolution, breaker state, effort table).
    Router {
        #[command(subcommand)]
        inner: RouterCommand,
    },
}

/// `caliban router <verb>` verbs.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum RouterCommand {
    /// Print the candidate list the router would resolve for a synthetic
    /// request, plus breaker state and effort knobs.
    Debug(router::RouterDebugArgs),
}

fn read_prompt(args: &Args) -> Result<String> {
    use std::io::Read as _;
    let raw = args
        .prompt_flag
        .as_deref()
        .or(args.prompt.as_deref())
        .context("no prompt given (use positional argument or --prompt)")?;
    if raw == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Ok(buf)
    } else {
        Ok(raw.to_string())
    }
}

fn build_provider(args: &Args) -> Result<Arc<dyn Provider + Send + Sync>> {
    use ProviderKind::{Anthropic, Google, Ollama, Openai};
    Ok(match args.provider {
        Anthropic => {
            use caliban_provider_anthropic::{AnthropicProvider, config::DirectConfig};
            Arc::new(AnthropicProvider::direct(
                DirectConfig::from_env().context("ANTHROPIC_API_KEY missing")?,
            )?)
        }
        Openai => {
            use caliban_provider_openai::{OpenAIProvider, config::DirectConfig};
            Arc::new(OpenAIProvider::direct(
                DirectConfig::from_env().context("OPENAI_API_KEY missing")?,
            )?)
        }
        Ollama => {
            use caliban_provider_ollama::{OllamaProvider, config::DirectConfig};
            Arc::new(OllamaProvider::direct(
                DirectConfig::from_env().unwrap_or_else(|_| DirectConfig::local()),
            )?)
        }
        Google => {
            use caliban_provider_google::{GoogleProvider, config::AIStudioConfig};
            Arc::new(GoogleProvider::ai_studio(
                AIStudioConfig::from_env().context("GEMINI_API_KEY missing")?,
            )?)
        }
    })
}

fn build_registry(
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
    r.register(Arc::new(BashTool::new(root.clone())));
    r.register(Arc::new(GlobTool::new(root.clone())));
    r.register(Arc::new(GrepTool::new(root)));
    r.register(Arc::new(WebFetchTool::new(web_fetch_client())));
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

/// Returns true if the user has opted out of the auto-memory feature.
/// Matches the loader-side check in `caliban_memory::loader`.
fn auto_memory_disabled() -> bool {
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
fn web_fetch_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!(
            "caliban/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/johnford2002/caliban)",
        ))
        .build()
        .expect("reqwest::Client default builder succeeds")
}

pub(crate) fn summarize(s: &str, max: usize) -> String {
    let one_line: String = s.lines().next().unwrap_or("").chars().take(max).collect();
    if s.lines().count() > 1 || s.chars().count() > max {
        format!("{one_line}\u{2026}")
    } else {
        one_line
    }
}

pub(crate) fn summarize_blocks(blocks: &[ContentBlock], max: usize) -> String {
    for b in blocks {
        if let ContentBlock::Text(t) = b {
            return summarize(&t.text, max);
        }
    }
    "(no text)".into()
}

async fn run_and_render(
    agent: Arc<Agent>,
    messages: Vec<Message>,
    cancel: CancellationToken,
    quiet: bool,
) -> Result<(Vec<Message>, Usage)> {
    use caliban_agent_core::TurnEvent;

    let mut stream = agent.stream_until_done(messages, cancel);
    let mut tool_inputs: HashMap<String, String> = HashMap::new();
    let mut at_column_zero = true;
    let mut final_messages: Vec<Message> = Vec::new();
    let mut total_usage = Usage::default();

    while let Some(event) = stream.next().await {
        match event? {
            TurnEvent::AssistantTextDelta { text, .. } => {
                print!("{text}");
                std::io::stdout().flush().ok();
                at_column_zero = text.ends_with('\n');
            }
            TurnEvent::AssistantThinkingDelta { text, .. } if !quiet => {
                eprint!("\x1b[2m{text}\x1b[0m");
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
                ..
            } => {
                if !at_column_zero {
                    println!();
                }
                if !quiet {
                    eprintln!(
                        "\n[caliban: {turn_count} turns \u{00b7} {}\u{2191} {}\u{2193} tokens]",
                        tu.input_tokens, tu.output_tokens
                    );
                }
                final_messages = fm;
                total_usage = tu;
                at_column_zero = true;
            }
            _ => {}
        }
    }

    if !at_column_zero {
        println!();
    }

    Ok((final_messages, total_usage))
}

/// Drive the agent loop in headless (`-p` / `--print`) mode and exit with
/// the appropriate process exit code (ADR 0025).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_headless(
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

    // Resolve the prompt: --print "x" wins, then --prompt, then positional,
    // then stdin (when --print was used with no value).
    let print_value = args.print.as_deref().filter(|s| !s.is_empty());
    let prompt_text = match (
        print_value,
        args.prompt_flag.as_deref(),
        args.prompt.as_deref(),
    ) {
        (Some(p), _, _) | (_, Some(p), _) | (_, _, Some(p)) => p.to_string(),
        (None, None, None) => {
            // No explicit prompt — read stdin (text or stream-json).
            let stdin_input = match headless::input::read_stdin_capped(&mut std::io::stdin()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[caliban] {e}");
                    return headless::exit_code_for(&e);
                }
            };
            if matches!(args.input_format, headless::InputFormat::StreamJson) {
                // Pick the first user frame as the prompt; emit a warning
                // to stderr if there are control frames (best-effort).
                match headless::input::parse_stream_json_payload(&stdin_input) {
                    Ok(frames) => {
                        let mut prompt = String::new();
                        for frame in frames {
                            if let headless::events::InputFrame::User { content } = frame {
                                prompt = headless::events::InputFrame::extract_text(&content);
                                break;
                            }
                        }
                        if prompt.is_empty() {
                            eprintln!("[caliban] no `user` frame found in stream-json stdin input");
                            return 66;
                        }
                        prompt
                    }
                    Err(e) => {
                        eprintln!("[caliban] {e}");
                        return headless::exit_code_for(&e);
                    }
                }
            } else {
                stdin_input.trim_end_matches('\n').to_string()
            }
        }
    };

    // Permission-prompt-tool: parsed-and-ignored with a warning (ADR 0023
    // Phase C will wire this).
    if let Some(tool) = &args.permission_prompt_tool {
        eprintln!(
            "[caliban] --permission-prompt-tool='{tool}' is accepted but inert; MCP elicitation lands with Phase C (ADR 0023)"
        );
    }

    // Budget warning: until OTel/cost lands, cost is always 0.0 — surface
    // a one-time warning when the operator passes --max-budget-usd.
    if args.max_budget_usd.is_some() {
        eprintln!(
            "[caliban] --max-budget-usd is in placeholder mode: every request contributes \
             0.0 USD until ADR 0033 wires real pricing"
        );
    }

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

    // System prompt: install (possibly empty) on a fresh session.
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
    messages.push(Message::user_text(prompt_text));

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
            tracing::warn!(target: "caliban::hooks", error = %e, "session_start hook error (non-fatal)");
        }
        // Flush any hook frames the sink captured before the run begins.
        let _ = driver.emit_init();
        let _ = driver.flush_hook_events();
    }

    let outcome = driver.run(Arc::clone(&agent), messages, cancel).await;

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
            tracing::warn!(target: "caliban::hooks", error = %e, "session_end hook error (non-fatal)");
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
            tracing::warn!(target: "caliban::sessions", error = %e, "session save failed");
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

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<()> {
    use std::io::IsTerminal as _;

    // Early dispatch: `caliban plugin <subcommand>` runs the plugin CLI and
    // exits, bypassing the agent loop. The dispatcher accepts the first
    // positional arg only — `caliban --debug plugin list` is not supported
    // (mirrors how Cargo subcommands work).
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() >= 2 && raw_args[1] == "plugin" {
        let code = plugin_cli::run(&raw_args[2..]).await;
        std::process::exit(code);
    }

    let args = Args::parse();

    // Diagnostic subcommands run before any provider construction or hook
    // wiring — they only need to read config.
    if let Some(CalibanCommand::Router { inner }) = &args.command {
        match inner {
            RouterCommand::Debug(dbg) => {
                let cwd = std::env::current_dir().context("could not get cwd")?;
                let out = router::run_debug(dbg, args.config_path.as_deref(), &cwd)?;
                print!("{out}");
                return Ok(());
            }
        }
    }

    // Install file-backed tracing subscriber when --debug or CALIBAN_DEBUG is set.
    let debug = args.debug || std::env::var("CALIBAN_DEBUG").is_ok();
    if debug {
        let log_path = dirs::cache_dir().map(|d| d.join("caliban").join("debug.log"));
        if let Some(path) = log_path {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let opened = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await;
            if let Ok(async_file) = opened {
                use tracing_subscriber::EnvFilter;
                use tracing_subscriber::layer::SubscriberExt as _;
                use tracing_subscriber::util::SubscriberInitExt as _;
                // tracing-subscriber's fmt layer wants std::io::Write, so
                // convert back to a std::fs::File. into_std offloads to the
                // blocking pool; safe here since this only runs once at start.
                let file = async_file.into_std().await;
                // Default filter keeps caliban + caliban_* crates at DEBUG and
                // silences noisy lower-level traces (mio, hyper, reqwest, …).
                // Users can override via RUST_LOG env var.
                let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    EnvFilter::new(
                        "debug,mio=warn,hyper=warn,hyper_util=warn,reqwest=warn,h2=warn,rustls=warn,tower=warn"
                    )
                });
                let layer = tracing_subscriber::fmt::layer()
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false);
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer)
                    .init();
                tracing::info!("caliban debug logging started — {}", path.display());
            }
        }
    }

    let workspace = match &args.workspace {
        Some(p) => WorkspaceRoot::new(p.clone()),
        None => WorkspaceRoot::current_dir().context("could not get cwd")?,
    };

    // Router v2: try caliban.toml first (--config flag or discovery), fall
    // back to the single-provider construction when no router config is
    // present (preserving v1 behavior). ADR 0038.
    let cwd_for_router = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let provider: Arc<dyn Provider + Send + Sync> =
        match router::try_load(args.config_path.as_deref(), &cwd_for_router)? {
            Some(wiring) => {
                tracing::info!(
                    target: "caliban::router",
                    path = %wiring.config_path.display(),
                    routes = wiring.router.routes().len(),
                    "model router wired from caliban.toml",
                );
                wiring.router
            }
            None => build_provider(&args)?,
        };
    let todos = caliban_agent_core::new_shared_todos();
    let plan_mode = caliban_agent_core::new_shared_plan_mode();

    // Discover plugins early (ADR 0030). Plugins contribute skill roots,
    // hooks config, MCP servers, agents, and output styles, so the manager
    // is constructed before any of those subsystems init. `--bare` and the
    // `--no-plugins` kill switch both produce an empty manager.
    let plugin_manager = if args.bare || args.no_plugins {
        caliban_plugins::PluginManager::default()
    } else {
        let ws_for_plugins = args
            .workspace
            .clone()
            .unwrap_or_else(|| workspace.root().to_path_buf());
        let roots = caliban_plugins::PluginRoots::default_for(&ws_for_plugins);
        let settings = caliban_plugins::PluginSettings::from_env();
        match caliban_plugins::PluginManager::load(&roots, &settings) {
            Ok(mgr) => {
                if !mgr.loaded().is_empty() {
                    tracing::info!(
                        target: "caliban::plugins",
                        count = mgr.loaded().len(),
                        "loaded plugins",
                    );
                }
                for f in mgr.failures() {
                    tracing::warn!(
                        target: "caliban::plugins",
                        path = %f.root_dir.display(),
                        error = %f.error,
                        "plugin failed to load",
                    );
                }
                mgr
            }
            Err(e) => {
                tracing::warn!(target: "caliban::plugins", error = %e, "plugin discovery failed; continuing without plugins");
                caliban_plugins::PluginManager::default()
            }
        }
    };
    let plugin_skill_roots = plugin_manager.skill_roots();

    let mut registry = build_registry(
        &args,
        workspace,
        Arc::clone(&todos),
        Arc::clone(&plan_mode),
        &plugin_skill_roots,
    );

    // MCP servers — Phase A: real spawn / handshake / list_tools (ADR 0023).
    // --bare (ADR 0025) suppresses MCP discovery entirely for reproducible CI.
    let mcp_summaries: Vec<caliban_mcp_client::ServerSummary> = if args.no_mcp || args.bare {
        Vec::new()
    } else {
        let ws_root_for_mcp = args.workspace.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        match caliban_mcp_client::load_config(&ws_root_for_mcp) {
            Ok(cfg) => match caliban_mcp_client::McpClientManager::start(&cfg).await {
                Ok(mgr) => {
                    mgr.register_all(&mut registry);
                    if mgr.enabled_count() > 0
                        || mgr.skipped_disabled() > 0
                        || mgr.failed_count() > 0
                    {
                        tracing::info!(
                            target: "caliban::mcp",
                            connected = mgr.enabled_count(),
                            failed = mgr.failed_count(),
                            disabled = mgr.skipped_disabled(),
                            "mcp manager started",
                        );
                    }
                    mgr.summaries().to_vec()
                }
                Err(e) => {
                    tracing::warn!(target: "caliban::mcp", error = %e, "mcp manager start failed; continuing without MCP");
                    Vec::new()
                }
            },
            Err(e) => {
                tracing::warn!(target: "caliban::mcp", error = %e, "mcp config load failed; continuing without MCP");
                Vec::new()
            }
        }
    };

    let model = args
        .model
        .clone()
        .unwrap_or_else(|| default_model_for(args.provider).to_string());

    // Wire AgentTool (sub-agent primitive). The factory closes over a
    // snapshot of the parent registry (which DOES NOT include AgentTool, so
    // sub-agents cannot recurse) + the parent's provider + chosen model.
    // Hook inheritance is deferred to v2 — sub-agents currently use NoopHooks.
    if !args.no_sub_agent && !args.no_tools {
        let snapshot_names: Vec<String> = registry.names().map(str::to_string).collect();
        let mut snapshot = ToolRegistry::new();
        for name in &snapshot_names {
            if let Some(t) = registry.get(name) {
                snapshot.register(Arc::clone(t));
            }
        }
        let provider_for_factory: Arc<dyn Provider + Send + Sync> = Arc::clone(&provider);
        let parent_model = model.clone();
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
        registry.register(Arc::new(AgentTool::new(factory, None)));
    }

    // Load hooks.toml (project + user scope). Empty config when missing or
    // when --no-hooks is set; the in-process PermissionsHook still runs.
    let workspace_root_for_hooks = args.workspace.clone().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    // --bare (ADR 0025) suppresses hooks.toml load entirely.
    let hooks_cfg = if args.no_hooks || args.bare {
        caliban_agent_core::HooksConfig::default()
    } else {
        match caliban_agent_core::HooksConfig::load(&workspace_root_for_hooks) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(target: "caliban::hooks", error = %e, "hooks.toml load failed; continuing with empty hooks config");
                caliban_agent_core::HooksConfig::default()
            }
        }
    };
    let hooks_cfg_summary = (
        hooks_cfg.total_handler_count(),
        hooks_cfg.disable_all_hooks || args.no_hooks,
    );

    let permissions_hook = if args.no_permissions {
        None
    } else {
        use caliban_agent_core::{
            Action, NonInteractiveAskHandler, NoopHooks, PermissionsHook, Rule, load_rules,
        };
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
        let workspace_root = args.workspace.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        let rules = load_rules(cli_rules, &workspace_root).context("loading permissions rules")?;
        let ask: Arc<dyn caliban_agent_core::AskHandler> = Arc::new(NonInteractiveAskHandler {
            auto_allow: args.auto_allow,
        });
        let hook: Arc<dyn caliban_agent_core::Hooks + Send + Sync> =
            Arc::new(PermissionsHook::new(rules, ask, Arc::new(NoopHooks)));
        Some(hook)
    };

    let mut builder = Agent::builder()
        .provider(provider)
        .tools(registry)
        .model(model.clone())
        .max_tokens(args.max_tokens)
        .max_turns(args.max_turns)
        .prompt_cache(!args.no_prompt_cache)
        .parallel_tools(!args.no_parallel_tools)
        .plan_mode(Arc::clone(&plan_mode));
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
    let hook_event_buffer = if args.include_hook_events {
        Some(headless::new_event_buffer())
    } else {
        None
    };
    {
        let mut layers: Vec<Arc<dyn caliban_agent_core::Hooks>> = Vec::new();
        if let Some(buf) = &hook_event_buffer {
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
    let agent = Arc::new(builder.build()?);

    // Fire SessionStart hook (best-effort).
    {
        let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let session_id = args.session.clone().unwrap_or_else(|| "ephemeral".into());
        let session_ctx = caliban_agent_core::SessionCtx {
            session_id: &session_id,
            cwd: &cwd_now,
            provider: provider_name(args.provider),
            model: &model,
        };
        if let Err(e) = agent.hooks().session_start(&session_ctx).await {
            tracing::warn!(target: "caliban::hooks", error = %e, "session_start hook error (non-fatal)");
        }
        let _ = hooks_cfg_summary; // silence unused when not later consumed
    }

    // Resolve session store (only when --session is given)
    let store = match (&args.sessions_dir, &args.session) {
        (_, None) => None,
        (Some(d), Some(_)) => Some(SessionStore::new(d.clone())),
        (None, Some(_)) => Some(SessionStore::new(SessionStore::default_root()?)),
    };

    // Load or create session
    let mut session = if let (Some(store), Some(name)) = (&store, &args.session) {
        Some(match store.load(name)? {
            Some(existing) => existing,
            None => {
                PersistedSession::new(name.clone(), provider_name(args.provider), model.clone())
            }
        })
    } else {
        None
    };

    // Seed the shared todos handle from the persisted session, if any.
    if let Some(sess) = session.as_ref() {
        todos
            .lock()
            .expect("todos lock poisoned")
            .clone_from(&sess.todos);
        plan_mode.store(sess.plan_mode, std::sync::atomic::Ordering::Relaxed);
    }

    // Resolve system prompt from CLI flags (or build default).
    let tool_names: Vec<&str> = agent.tools().names().collect();
    let cwd_for_prompt = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let default_prompt_in_effect =
        args.system.is_none() && args.system_file.is_none() && !args.no_system;
    let system_prompt = system_prompt::resolve(
        args.system.as_deref(),
        args.system_file.as_deref(),
        args.no_system,
        &cwd_for_prompt,
        &tool_names,
        args.no_tools,
    )?;

    // Load memory tiers and splice into the default system prompt, then
    // wrap with the active output-style block (after memory, before the
    // base body). The operator's --system / --system-file / --no-system
    // always wins — those paths intentionally skip both memory and output
    // styles.
    let system_prompt = if default_prompt_in_effect && let Some(body) = system_prompt {
        let workspace_root = args
            .workspace
            .clone()
            .unwrap_or_else(|| cwd_for_prompt.clone());

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
            let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);
            match caliban_memory::load(&cfg).await {
                Ok(prefix) => prefix.splice_into(&with_style),
                Err(e) => {
                    tracing::warn!(target: "caliban::memory", error = %e, "memory load failed; using default prompt without memory");
                    with_style
                }
            }
        };
        Some(final_prompt)
    } else {
        system_prompt
    };

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

    // --- Headless / print-mode dispatch (ADR 0025).
    // Triggered explicitly by -p / --print, or by --output-format. Other
    // flags (--max-budget-usd, --bare, --json-schema, --include-…, etc.)
    // are only meaningful in headless mode but do not by themselves switch
    // drivers — operators must opt in via -p / --output-format.
    let headless_active = args.print.is_some() || args.output_format.is_some();
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
        let exit = run_headless(
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
        )
        .await;
        std::process::exit(exit);
    }
    // hook_event_buffer is consumed by headless mode; for the TUI/interactive
    // path the buffer is dropped here (the sink still runs but its frames
    // are unused — informational).
    drop(hook_event_buffer);

    // --- TUI dispatch: no prompt + stdin is a TTY → enter interactive TUI.
    let has_prompt = args.prompt.is_some() || args.prompt_flag.is_some();
    let stdin_is_tty = std::io::stdin().is_terminal();
    if !has_prompt {
        if stdin_is_tty {
            return tui::run(
                args,
                agent,
                store,
                session,
                system_prompt,
                todos,
                plan_mode,
                mcp_summaries,
            )
            .await;
        }
        anyhow::bail!(
            "no prompt given and stdin is not a TTY; use --prompt or pass a positional argument"
        );
    }

    // --- Single-prompt path: register the outer Ctrl-C handler.
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

    let prompt = read_prompt(&args)?;

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

    let (final_messages, total_usage) =
        run_and_render(Arc::clone(&agent), messages, cancel, args.quiet).await?;

    // Fire SessionEnd hook (best-effort).
    {
        let cwd_now = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let session_id = args.session.clone().unwrap_or_else(|| "ephemeral".into());
        let session_ctx = caliban_agent_core::SessionCtx {
            session_id: &session_id,
            cwd: &cwd_now,
            provider: provider_name(args.provider),
            model: &model,
        };
        let outcome = caliban_agent_core::SessionOutcome {
            turn_count: 0, // not tracked at this layer; populated from final_messages by callers.
            input_tokens: total_usage.input_tokens,
            output_tokens: total_usage.output_tokens,
        };
        if let Err(e) = agent.hooks().session_end(&session_ctx, &outcome).await {
            tracing::warn!(target: "caliban::hooks", error = %e, "session_end hook error (non-fatal)");
        }
    }

    // Save session back if requested
    if let (Some(store), Some(ref mut s)) = (store.as_ref(), session.as_mut())
        && !args.no_save
    {
        s.merge_run(final_messages, total_usage);
        // Snapshot the shared todo handle back into the persisted session.
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

    Ok(())
}
