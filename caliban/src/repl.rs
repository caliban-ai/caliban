//! Interactive REPL mode.
//!
//! Entered when `caliban` is invoked with no prompt argument and stdin is a TTY.
//! Uses `rustyline` for line editing and history. Slash commands route to
//! [`handle_command`]. Each turn spawns a per-turn Ctrl-C handler that cancels
//! only that turn, returning to the prompt rather than exiting.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use caliban_agent_core::{Agent, Message, TurnEvent};
use caliban_provider::Usage;
use caliban_sessions::{PersistedSession, SessionStore};
use futures::StreamExt as _;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};
use tokio_util::sync::CancellationToken;

use crate::{Args, default_model_for, summarize, summarize_blocks};

#[allow(dead_code)] // RunPrompt reserved for future /load or macro commands
enum CommandOutcome {
    Continue,
    Exit,
    RunPrompt(String),
}

/// Run the interactive REPL loop.
///
/// # Errors
/// Propagates from agent runs and session-store I/O.
pub(crate) async fn run(
    args: Args,
    agent: Arc<Agent>,
    store: Option<SessionStore>,
    mut session: Option<PersistedSession>,
) -> Result<()> {
    let history_path = dirs::data_dir().map(|d| d.join("caliban").join("repl_history.txt"));

    let mut rl: Editor<(), FileHistory> =
        Editor::with_config(Config::builder().auto_add_history(true).build())?;
    if let Some(p) = &history_path {
        let _ = rl.load_history(p); // best effort
    }

    print_banner(&args, session.as_ref());

    loop {
        let line = match rl.readline("> ") {
            Ok(line) => line,
            Err(ReadlineError::Eof) => break,
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C at the prompt — polite exit
                break;
            }
            Err(e) => return Err(e.into()),
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('/') {
            match handle_command(trimmed, store.as_ref(), session.as_mut()) {
                CommandOutcome::Continue => continue,
                CommandOutcome::Exit => break,
                CommandOutcome::RunPrompt(p) => {
                    run_one_turn(&agent, &mut session, &args, &p).await?;
                }
            }
        } else {
            run_one_turn(&agent, &mut session, &args, trimmed).await?;
        }

        // Auto-save after each turn when a session store is active.
        if let (Some(st), Some(s)) = (store.as_ref(), session.as_ref()) {
            if !args.no_save {
                if let Err(e) = st.save(s) {
                    eprintln!("[caliban: session save error: {e}]");
                }
            }
        }
    }

    // Persist history file on exit.
    if let Some(p) = &history_path {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let _ = rl.save_history(p);
    }

    // Final session save on exit.
    if let (Some(st), Some(s)) = (store.as_ref(), session.as_ref()) {
        if !args.no_save {
            st.save(s).context("save session on exit")?;
            if !args.quiet {
                eprintln!("[caliban: saved session '{}']", s.name);
            }
        }
    }

    Ok(())
}

fn print_banner(args: &Args, session: Option<&PersistedSession>) {
    let version = env!("CARGO_PKG_VERSION");
    let provider = format!("{:?}", args.provider).to_lowercase();
    let model = args
        .model
        .as_deref()
        .unwrap_or_else(|| default_model_for(args.provider));
    let session_info = session
        .map(|s| {
            format!(
                " \u{2014} session: {} ({} turns, {}k tokens)",
                s.name,
                s.turn_count(),
                (s.total_usage.input_tokens + s.total_usage.output_tokens) / 1000
            )
        })
        .unwrap_or_default();
    println!("caliban v{version} \u{2014} {provider} {model}{session_info}");
    println!("Type your message; /help for commands; /exit or Ctrl-D to quit.");
    println!();
}

fn handle_command(
    line: &str,
    store: Option<&SessionStore>,
    session: Option<&mut PersistedSession>,
) -> CommandOutcome {
    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();

    match cmd {
        "/help" => {
            println!("Commands:");
            println!("  /help                \u{2014} show this help");
            println!("  /exit, /quit         \u{2014} save and exit");
            println!("  /clear               \u{2014} clear in-memory history (does not save)");
            println!("  /sessions            \u{2014} list saved sessions");
            println!(
                "  /load <name>         \u{2014} load a named session (v1: not yet implemented)"
            );
            println!("  /save [<name>]       \u{2014} save current session (optionally rename)");
            println!("  /usage               \u{2014} show accumulated usage");
            println!("Anything not starting with / is sent as a user prompt.");
            CommandOutcome::Continue
        }
        "/exit" | "/quit" => CommandOutcome::Exit,
        "/clear" => {
            if let Some(s) = session {
                s.messages.clear();
                println!("[history cleared]");
            } else {
                println!("[no session active]");
            }
            CommandOutcome::Continue
        }
        "/sessions" => {
            match store {
                Some(s) => match s.list() {
                    Ok(list) if list.is_empty() => println!("[no sessions yet]"),
                    Ok(list) => {
                        for m in &list {
                            println!(
                                "  {} \u{2014} {} turns, {} tokens \u{2014} {}",
                                m.name,
                                m.turn_count,
                                m.total_tokens,
                                m.updated_at.format("%Y-%m-%d %H:%M:%S")
                            );
                        }
                    }
                    Err(e) => println!("[error listing sessions: {e}]"),
                },
                None => println!("[no session store configured]"),
            }
            CommandOutcome::Continue
        }
        "/usage" => {
            match session {
                Some(s) => println!(
                    "session {}: {} turns, {} input + {} output tokens",
                    s.name,
                    s.turn_count(),
                    s.total_usage.input_tokens,
                    s.total_usage.output_tokens
                ),
                None => println!("[no session active]"),
            }
            CommandOutcome::Continue
        }
        "/save" => {
            if let (Some(st), Some(s)) = (store, session) {
                let target_name = if arg.is_empty() {
                    s.name.clone()
                } else {
                    arg.to_string()
                };
                if target_name == s.name {
                    match st.save(s) {
                        Ok(()) => println!("[saved]"),
                        Err(e) => println!("[save error: {e}]"),
                    }
                } else {
                    let mut renamed = s.clone();
                    renamed.name = target_name;
                    match st.save(&renamed) {
                        Ok(()) => println!("[saved as '{}']", renamed.name),
                        Err(e) => println!("[save error: {e}]"),
                    }
                }
            } else {
                println!("[no session to save]");
            }
            CommandOutcome::Continue
        }
        "/load" => {
            // V1 limitation: /load requires reinitializing REPL state which is not yet
            // supported. Users can /exit and reinvoke with --session <name>.
            println!("[/load not yet implemented in v1; /exit and reinvoke with --session {arg}]");
            CommandOutcome::Continue
        }
        unknown => {
            println!("Unknown command: {unknown}. Type /help.");
            CommandOutcome::Continue
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run_one_turn(
    agent: &Arc<Agent>,
    session: &mut Option<PersistedSession>,
    args: &Args,
    prompt: &str,
) -> Result<()> {
    let mut messages: Vec<Message> = session
        .as_ref()
        .map(|s| s.messages.clone())
        .unwrap_or_default();
    messages.push(Message::user_text(prompt.to_string()));

    // Spawn a per-turn Ctrl-C handler so that Ctrl-C during a turn cancels the
    // turn and returns to the prompt rather than exiting the whole process.
    let per_turn_cancel = CancellationToken::new();
    let cancel_handle = per_turn_cancel.clone();
    let ctrl_c_task = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_handle.cancel();
    });

    let mut stream = Arc::clone(agent).stream_until_done(messages, per_turn_cancel.clone());

    let mut tool_inputs: HashMap<String, String> = HashMap::new();
    let mut at_column_zero = true;
    let mut final_messages: Vec<Message> = Vec::new();
    let mut total_usage = Usage::default();

    while let Some(event) = stream.next().await {
        match event {
            Err(caliban_agent_core::Error::Cancelled) => {
                eprintln!("\n[cancelled]");
                ctrl_c_task.abort();
                return Ok(()); // return to prompt
            }
            Err(e) => {
                eprintln!("\n[error: {e}]");
                ctrl_c_task.abort();
                return Ok(()); // surface error to prompt, keep REPL alive
            }
            Ok(ev) => match ev {
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
                    tool_inputs.insert(tool_use_id, String::new());
                    eprint!("\u{1F527} {name}(");
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
                    final_messages: fm,
                    total_usage: tu,
                    turn_count,
                    ..
                } => {
                    if !at_column_zero {
                        println!();
                    }
                    if !args.quiet {
                        eprintln!(
                            "[caliban: {turn_count} turns \u{00b7} {}\u{2191} {}\u{2193} tokens]",
                            tu.input_tokens, tu.output_tokens
                        );
                    }
                    final_messages = fm;
                    total_usage = tu;
                    at_column_zero = true;
                }
                _ => {}
            },
        }
    }

    ctrl_c_task.abort();

    if !at_column_zero {
        println!();
    }

    if let Some(s) = session {
        s.merge_run(final_messages, total_usage);
    }

    Ok(())
}
