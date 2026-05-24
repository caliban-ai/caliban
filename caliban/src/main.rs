//! caliban — agent harness binary.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::multiple_crate_versions)]

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
use caliban_tools_builtin::{
    BashTool, EditTool, EnterPlanModeTool, ExitPlanModeTool, GlobTool, GrepTool, ReadTool,
    TodoWriteTool, WebFetchTool, WorkspaceRoot, WriteTool,
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
    #[arg(short = 'p', long = "prompt", value_name = "PROMPT")]
    pub(crate) prompt_flag: Option<String>,

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
) -> ToolRegistry {
    if args.no_tools {
        return ToolRegistry::new();
    }
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
    r
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

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<()> {
    use std::io::IsTerminal as _;

    let args = Args::parse();

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

    let provider = build_provider(&args)?;
    let todos = caliban_agent_core::new_shared_todos();
    let plan_mode = caliban_agent_core::new_shared_plan_mode();
    let registry = build_registry(&args, workspace, Arc::clone(&todos), Arc::clone(&plan_mode));

    let model = args
        .model
        .clone()
        .unwrap_or_else(|| default_model_for(args.provider).to_string());

    let mut builder = Agent::builder()
        .provider(provider)
        .tools(registry)
        .model(model.clone())
        .max_tokens(args.max_tokens)
        .max_turns(args.max_turns)
        .prompt_cache(!args.no_prompt_cache)
        .parallel_tools(!args.no_parallel_tools)
        .plan_mode(Arc::clone(&plan_mode));
    if let Some(limit) = args.parallel_tool_limit {
        builder = builder.parallel_tool_limit(limit);
    }
    if let Some(t) = args.temperature {
        builder = builder.temperature(t);
    }
    let agent = Arc::new(builder.build()?);

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

    // Load memory tiers and splice into the default system prompt. The
    // operator's --system / --system-file / --no-system always wins — those
    // paths intentionally skip memory.
    let system_prompt = if default_prompt_in_effect && let Some(body) = system_prompt {
        let workspace_root = args
            .workspace
            .clone()
            .unwrap_or_else(|| cwd_for_prompt.clone());
        let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);
        match caliban_memory::load(&cfg).await {
            Ok(prefix) => Some(prefix.splice_into(&body)),
            Err(e) => {
                tracing::warn!(target: "caliban::memory", error = %e, "memory load failed; using default prompt without memory");
                Some(body)
            }
        }
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

    // --- TUI dispatch: no prompt + stdin is a TTY → enter interactive TUI.
    let has_prompt = args.prompt.is_some() || args.prompt_flag.is_some();
    let stdin_is_tty = std::io::stdin().is_terminal();
    if !has_prompt {
        if stdin_is_tty {
            return tui::run(args, agent, store, session, system_prompt, todos, plan_mode).await;
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
