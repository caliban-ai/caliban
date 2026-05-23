//! caliban — agent harness binary.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::multiple_crate_versions)]

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use caliban_agent_core::{Agent, Message, ToolRegistry};
use caliban_provider::{ContentBlock, Provider};
use caliban_tools_builtin::{
    BashTool, EditTool, GlobTool, GrepTool, ReadTool, WorkspaceRoot, WriteTool,
};
use clap::{Parser, ValueEnum};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProviderKind {
    Anthropic,
    Openai,
    Ollama,
    Google,
}

fn default_model_for(p: ProviderKind) -> &'static str {
    match p {
        ProviderKind::Anthropic => "claude-3-5-sonnet",
        ProviderKind::Openai => "gpt-4o",
        ProviderKind::Ollama => "llama3.1",
        ProviderKind::Google => "gemini-2.0-flash",
    }
}

#[derive(Debug, Parser)]
#[command(name = "caliban", version, about = "caliban agent harness")]
struct Args {
    /// User prompt. Use "-" to read from stdin.
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,

    /// Alternative way to specify the prompt
    #[arg(short = 'p', long = "prompt", value_name = "PROMPT")]
    prompt_flag: Option<String>,

    /// Which provider to use
    #[arg(long, value_enum, default_value_t = ProviderKind::Anthropic)]
    provider: ProviderKind,

    /// Model name (defaults per provider)
    #[arg(long)]
    model: Option<String>,

    /// Per-turn output token limit
    #[arg(long, default_value_t = 2048)]
    max_tokens: u32,

    /// Maximum agent loop iterations
    #[arg(long, default_value_t = 50)]
    max_turns: u32,

    /// Sampling temperature
    #[arg(long)]
    temperature: Option<f32>,

    /// Workspace root for file/shell tools
    #[arg(long)]
    workspace: Option<PathBuf>,

    /// Disable all tools (chat-only mode)
    #[arg(long)]
    no_tools: bool,

    /// Reject tool paths outside the workspace root
    #[arg(long)]
    restrict_paths: bool,

    /// Suppress tool-execution announcements
    #[arg(long)]
    quiet: bool,
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

fn summarize(s: &str, max: usize) -> String {
    let one_line: String = s.lines().next().unwrap_or("").chars().take(max).collect();
    if s.lines().count() > 1 || s.chars().count() > max {
        format!("{one_line}\u{2026}")
    } else {
        one_line
    }
}

fn summarize_blocks(blocks: &[ContentBlock], max: usize) -> String {
    for b in blocks {
        if let ContentBlock::Text(t) = b {
            return summarize(&t.text, max);
        }
    }
    "(no text)".into()
}

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let workspace = match &args.workspace {
        Some(p) => WorkspaceRoot::new(p.clone()),
        None => WorkspaceRoot::current_dir().context("could not get cwd")?,
    };
    let prompt = read_prompt(&args)?;
    let provider = build_provider(&args)?;
    let registry = build_registry(&args, workspace);

    let model = args
        .model
        .unwrap_or_else(|| default_model_for(args.provider).to_string());

    let mut builder = Agent::builder()
        .provider(provider)
        .tools(registry)
        .model(model)
        .max_tokens(args.max_tokens)
        .max_turns(args.max_turns);
    if let Some(t) = args.temperature {
        builder = builder.temperature(t);
    }
    let agent = Arc::new(builder.build()?);

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

    let messages = vec![Message::user_text(prompt)];
    let mut stream = Arc::clone(&agent).stream_until_done(messages, cancel);

    let mut tool_inputs: HashMap<String, String> = HashMap::new();
    let mut at_column_zero = true;

    while let Some(event) = stream.next().await {
        use caliban_agent_core::TurnEvent;
        match event? {
            TurnEvent::AssistantTextDelta { text, .. } => {
                print!("{text}");
                std::io::stdout().flush().ok();
                at_column_zero = text.ends_with('\n');
            }
            TurnEvent::AssistantThinkingDelta { text, .. } if !args.quiet => {
                eprint!("\x1b[2m{text}\x1b[0m");
            }
            TurnEvent::ToolCallStart {
                tool_use_id, name, ..
            } if !args.quiet => {
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
            } if !args.quiet => {
                let input_str = tool_inputs.remove(&tool_use_id).unwrap_or_default();
                let input_summary = summarize(&input_str, 80);
                let result_summary = summarize_blocks(&content, 80);
                let prefix = if is_error { "(error) " } else { "" };
                eprintln!("{input_summary})");
                eprintln!("   \u{2192} {prefix}{result_summary}");
                at_column_zero = true;
            }
            TurnEvent::RunEnd {
                total_usage,
                turn_count,
                ..
            } if !args.quiet => {
                if !at_column_zero {
                    println!();
                }
                eprintln!(
                    "\n[caliban: {turn_count} turns \u{00b7} {}\u{2191} {}\u{2193} tokens]",
                    total_usage.input_tokens, total_usage.output_tokens
                );
            }
            _ => {}
        }
    }

    if !at_column_zero {
        println!();
    }
    Ok(())
}
