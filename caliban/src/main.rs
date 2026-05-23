//! caliban — agent harness binary.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::multiple_crate_versions)]

mod tui;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use caliban_agent_core::{Agent, Message, ToolRegistry};
use caliban_provider::{ContentBlock, Provider, Usage};
use caliban_sessions::{PersistedSession, SessionStore};
use caliban_tools_builtin::{
    BashTool, EditTool, GlobTool, GrepTool, ReadTool, WorkspaceRoot, WriteTool,
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

fn build_registry(args: &Args, workspace: WorkspaceRoot) -> ToolRegistry {
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
    r
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

    let workspace = match &args.workspace {
        Some(p) => WorkspaceRoot::new(p.clone()),
        None => WorkspaceRoot::current_dir().context("could not get cwd")?,
    };

    let provider = build_provider(&args)?;
    let registry = build_registry(&args, workspace);

    let model = args
        .model
        .clone()
        .unwrap_or_else(|| default_model_for(args.provider).to_string());

    let mut builder = Agent::builder()
        .provider(provider)
        .tools(registry)
        .model(model.clone())
        .max_tokens(args.max_tokens)
        .max_turns(args.max_turns);
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
    let session = if let (Some(store), Some(name)) = (&store, &args.session) {
        Some(match store.load(name)? {
            Some(existing) => existing,
            None => {
                PersistedSession::new(name.clone(), provider_name(args.provider), model.clone())
            }
        })
    } else {
        None
    };

    // --- TUI dispatch: no prompt + stdin is a TTY → enter interactive TUI.
    let has_prompt = args.prompt.is_some() || args.prompt_flag.is_some();
    let stdin_is_tty = std::io::stdin().is_terminal();
    if !has_prompt {
        if stdin_is_tty {
            return tui::run(args, agent, store, session).await;
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

    // Build initial messages: prior session history + new user prompt
    let mut messages = session
        .as_ref()
        .map(|s| s.messages.clone())
        .unwrap_or_default();
    messages.push(Message::user_text(prompt));

    let mut session = session;
    let (final_messages, total_usage) =
        run_and_render(Arc::clone(&agent), messages, cancel, args.quiet).await?;

    // Save session back if requested
    if let (Some(store), Some(ref mut s)) = (store.as_ref(), session.as_mut()) {
        if !args.no_save {
            s.merge_run(final_messages, total_usage);
            store.save(s)?;
            if !args.quiet {
                eprintln!(
                    "[caliban: saved session '{}' ({} turns, {} tokens)]",
                    s.name,
                    s.turn_count(),
                    s.total_usage.input_tokens + s.total_usage.output_tokens,
                );
            }
        }
    }

    Ok(())
}
