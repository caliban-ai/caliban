//! Run-driver functions for the `caliban` binary.
//!
//! Hosts the agent-loop drivers shared between the single-prompt CLI and
//! headless (`-p` / `--print`) dispatch paths:
//!
//! - [`run_and_render`] — single-prompt agent driver.
//! - [`run_headless`] — `-p` / `--print` agent driver.
//! - [`run_single_prompt`] — single-prompt path (no `-p`, no TUI).

use std::io::Write as _;
use std::sync::Arc;

use anyhow::Result;
use caliban_agent_core::{Agent, Message};
use caliban_provider::Usage;
use caliban_sessions::{PersistedSession, SessionStore};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

use super::compose::fire_session_end;
use crate::args::{Args, provider_name, resolved_provider, summarize, summarize_blocks};
use crate::{headless, system_prompt};

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_and_render(
    agent: Arc<Agent>,
    messages: Vec<Message>,
    cancel: CancellationToken,
    quiet: bool,
) -> Result<(Vec<Message>, Usage, caliban_agent_core::StopCondition)> {
    use caliban_agent_core::TurnEvent;

    let requested_model = agent.active_model().as_str().to_string();
    let mut decoder = crate::stream_decode::StreamDecoder::new();
    let mut stream = agent.stream_until_done(messages, cancel);
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
            TurnEvent::TurnStart { model: actual, .. } => {
                // F4: surface silent model substitution by LM Studio /
                // similar OpenAI-compatible servers. The response's `model`
                // field is the actually-served model ID; warn once per pair
                // (the shared decoder dedups so a multi-turn run doesn't spam).
                if let Some(warning) = decoder.model_mismatch_warning(&requested_model, &actual) {
                    eprintln!("{warning}");
                }
            }
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
                decoder.tool_started(tool_use_id.clone(), name.clone());
                eprint!("\u{1f527} {name}(");
            }
            TurnEvent::ToolCallInputDelta {
                tool_use_id,
                partial_json,
                ..
            } => {
                decoder.tool_input_delta(tool_use_id, &partial_json);
            }
            TurnEvent::ToolCallEnd {
                tool_use_id,
                is_error,
                content,
                ..
            } if !quiet => {
                let input_str = decoder
                    .take_tool_input(&tool_use_id)
                    .map(|t| t.json)
                    .unwrap_or_default();
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
                ..
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
///
/// `MaxTurnsReached` returns `75` (`EX_TEMPFAIL`) — distinct from the
/// `128 + signal` UNIX convention so CI scripts can tell a max-turns
/// stop from a real `SIGINT` (F12 follow-up). Stays in sync with
/// `headless::exit_code_for`.
pub(crate) fn stop_condition_exit_code(stop: &caliban_agent_core::StopCondition) -> i32 {
    use caliban_agent_core::StopCondition;
    match stop {
        StopCondition::EndOfTurn => 0,
        StopCondition::MaxTurnsReached(_) => 75,
        StopCondition::Cancelled => 124,
        StopCondition::ProviderError(_)
        | StopCondition::HookDenied(_)
        | StopCondition::CompactionFailed(_)
        | StopCondition::Refusal(_)
        | StopCondition::ContentFilter(_)
        | StopCondition::MaxTokensExhausted
        | StopCondition::StreamIdle(_)
        | StopCondition::ThinkingBudgetExhausted => 1,
    }
}

/// Map a [`caliban_agent_core::StopCondition`] to a one-line stderr
/// surface for the single-prompt CLI driver. Returns `None` for the
/// natural `EndOfTurn` stop (no surfacing needed). Mirrors the TUI and
/// headless drivers' surfacing of the lmstudio probe's Findings 5 + 9,
/// closing the previously-missed `run_and_render` path.
///
/// Delegates to the canonical [`caliban_agent_core::StopCondition::surface`]
/// (this driver wants only the line, not the `level` color hint), so the
/// single-prompt CLI shares the exact wording with the TUI and headless
/// drivers instead of keeping a third drifted copy (#154).
fn stopped_for_surface_line(stopped_for: &caliban_agent_core::StopCondition) -> Option<String> {
    stopped_for.surface().map(|s| s.line)
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

/// Assemble the leading `System` message for a headless run.
///
/// A fresh run has no system message yet, so the base system prompt and the
/// optional `--json-schema` directive are combined into a new leading `System`
/// message. A sessioned/continued run already carries a `System` message
/// (rebuilt in `main.rs` from the system prompt + todos), so the schema
/// directive must be *appended* to it — skipping it there is what made
/// `--json-schema` silently degrade to validate-only under
/// `--session`/`--continue`/`--resume` (#214, gap in #174).
fn apply_system_and_schema(
    messages: &mut Vec<Message>,
    base_system: Option<String>,
    schema_instruction: Option<String>,
) {
    let has_system = messages
        .first()
        .is_some_and(|m| m.role == caliban_provider::Role::System);
    if has_system {
        // The base prompt is already present in the existing system message;
        // only the schema directive may be missing. Append it to the first
        // text block so the model is actually instructed to emit JSON (#214).
        if let Some(instruction) = schema_instruction {
            append_to_system_message(messages, &instruction);
        }
        return;
    }
    let combined = match (base_system, schema_instruction) {
        (Some(b), Some(s)) => Some(format!("{b}\n\n{s}")),
        (Some(b), None) => Some(b),
        (None, Some(s)) => Some(s),
        (None, None) => None,
    };
    if let Some(text) = combined {
        messages.insert(0, Message::system_text(text));
    }
}

/// Append `instruction` to the first text block of the leading message,
/// matching the `\n\n` join the fresh-run path uses. Falls back to inserting a
/// new text block if the leading message has no text block.
fn append_to_system_message(messages: &mut [Message], instruction: &str) {
    let Some(first) = messages.first_mut() else {
        return;
    };
    for block in &mut first.content {
        if let caliban_provider::ContentBlock::Text(tb) = block {
            tb.text = format!("{}\n\n{instruction}", tb.text);
            return;
        }
    }
    first.content.insert(
        0,
        caliban_provider::ContentBlock::Text(caliban_provider::TextBlock {
            text: instruction.to_string(),
            cache_control: None,
        }),
    );
}

/// Resolve `--continue` / `--resume <NAME>` for either driver.
///
/// When a resume flag is set, picks the session via
/// [`headless::session_loader::resolve_session`] and replays its todos +
/// plan-mode into the shared handles, returning the loaded session. Returns
/// `Ok(None)` when no resume flag is set or no matching session is found, so
/// the caller keeps whatever `session` it already had.
///
/// The store is reused from `store` when present, else derived from
/// [`SessionStore::default_root`]; a `default_root` failure is surfaced as
/// [`headless::HeadlessError::SessionLoad`] so both drivers route it through
/// the shared ADR-0025 exit-code table (#165).
fn resolve_resume(
    args: &Args,
    store: Option<&SessionStore>,
    todos: &caliban_agent_core::SharedTodos,
    plan_mode: &caliban_agent_core::SharedPlanMode,
) -> std::result::Result<Option<PersistedSession>, headless::HeadlessError> {
    if !(args.continue_latest || args.resume.is_some()) {
        return Ok(None);
    }
    let store_for_resume = match store {
        Some(s) => s.clone(),
        None => SessionStore::new(
            SessionStore::default_root()
                .map_err(|e| headless::HeadlessError::SessionLoad(e.to_string()))?,
        ),
    };
    match headless::session_loader::resolve_session(
        &store_for_resume,
        args.continue_latest,
        args.resume.as_deref(),
    )? {
        Some(s) => {
            // Replay todos / plan-mode from the resumed session.
            todos.lock().expect("todos lock").clone_from(&s.todos);
            plan_mode.store(s.plan_mode, std::sync::atomic::Ordering::Relaxed);
            Ok(Some(s))
        }
        None => Ok(None),
    }
}

/// Replay the live todo + plan-mode handles into `session` and persist it via
/// `store`. Shared save tail of both drivers; the run-message / usage merge and
/// any post-save reporting stay caller-specific (the single-prompt path merges
/// the agent-core `Usage` accumulator and prints a summary; the headless path
/// merges budget-tracked totals and logs failures rather than failing) (#165).
fn persist_session(
    session: &mut PersistedSession,
    store: &SessionStore,
    todos: &caliban_agent_core::SharedTodos,
    plan_mode: &caliban_agent_core::SharedPlanMode,
) -> caliban_sessions::Result<()> {
    session
        .todos
        .clone_from(&*todos.lock().expect("todos lock poisoned"));
    session.plan_mode = plan_mode.load(std::sync::atomic::Ordering::Relaxed);
    store.save(session)
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
    permission_mode: caliban_agent_core::PermissionMode,
) -> i32 {
    let output_format = args.output_format.unwrap_or(headless::OutputFormat::Text);

    // Resolve --continue / --resume. They override the in-memory `session`
    // computed by the legacy `--session` flag when both are present.
    let mut session = session;
    match resolve_resume(args, store.as_ref(), &todos, &plan_mode) {
        Ok(Some(s)) => session = Some(s),
        Ok(None) => {}
        Err(e) => {
            eprintln!("[caliban] {e}");
            return headless::exit_code_for(&e);
        }
    }

    // Resolve the prompt source. Four shapes:
    // - An explicit CLI prompt (`--print "x"` / `--prompt` / positional) →
    //   single-frame path; `prompt_source` is `Single(text)`.
    // - A prompt slot set to the `-` sentinel → read stdin. Routes to
    //   `StreamJson` when `--input-format stream-json`, else `Single`.
    //   This pairs with the clap-time validator that rejects any
    //   non-`-` inline prompt in stream-json mode (lmstudio Finding 13).
    // - No explicit prompt, plain-text stdin → single-frame path with
    //   stdin contents as the prompt.
    // - No explicit prompt, `--input-format stream-json` → multi-frame
    //   path; `prompt_source` is `StreamJson(stdin_input)` and is
    //   driven below by `HeadlessDriver::run_frames` (Finding 10).
    let print_value = args.print.as_deref().filter(|s| !s.is_empty());
    // First pick the first non-empty prompt slot. If it's `-`, treat as
    // "delegate to stdin" — same semantics as omitting the flag.
    let inline_prompt = print_value
        .or(args.prompt_flag.as_deref())
        .or(args.prompt.as_deref());
    let prompt_source = match inline_prompt {
        Some(p) if p != "-" => PromptSource::Single(p.to_string()),
        // Either no inline prompt, or the `-` sentinel: pull stdin and
        // route by --input-format.
        _ => {
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
    // Assemble the leading system message. The base prompt is only needed when
    // no system message exists yet; the --json-schema directive must apply in
    // both cases (a sessioned run already has a system message — #174/#214).
    let base_system = system_prompt
        .as_ref()
        .map(|sp| system_prompt::append_todo_block(sp, &todo_snapshot));
    let schema_instruction = json_schema.as_ref().map(headless::JsonSchema::instruction);
    apply_system_and_schema(&mut messages, base_system, schema_instruction);
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

    let model_summary = format!("{}/{}", provider_name(resolved_provider(args)), model);
    let session_id = args
        .session
        .clone()
        .or_else(|| args.resume.clone())
        .unwrap_or_else(|| "ephemeral".into());

    let budget = headless::BudgetTracker::new(args.max_budget_usd);

    // Resolved permission_mode string for the `system/init` frame. The
    // literal `"disabled"` distinguishes `--no-permissions` (no hook at
    // all) from the camelCase ADR 0029 mode names. lmstudio Finding 15.
    let permission_mode_str = if args.no_permissions {
        "disabled".to_string()
    } else {
        permission_mode.as_str().to_string()
    };

    let config = headless::HeadlessRunConfig {
        output_format,
        input_format: args.input_format,
        // Enforce the turn cap on every headless path (`run_headless` only
        // runs when headless is active — explicit `-p`/`--output-format` *or*
        // auto-headless). Gating on `-p`/`--output-format` alone left the
        // auto-headless path without the clean `--max-turns 0` short-circuit,
        // so identical commands diverged on whether `-p` was typed (#184 HL2).
        max_turns: Some(args.max_turns),
        budget: Arc::clone(&budget),
        json_schema,
        include_partial_messages: args.include_partial_messages,
        include_hook_events: args.include_hook_events,
        replay_user_messages: args.replay_user_messages,
        verbose: args.verbose,
        bare_mode: args.bare,
        fallback_model: args.fallback_model.clone(),
        session_id,
        setting_sources,
        tools,
        plugins: plugin_descriptors,
        model_summary,
        requested_model: model.clone(),
        cwd,
        hook_buffer: hook_event_buffer,
        permission_mode: permission_mode_str,
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
            provider: provider_name(resolved_provider(args)),
            model: &model,
        };
        // Event-emission only: context injection already happened at the
        // main.rs SessionStart fire (threaded into the system prompt via
        // `resolve_system_prompt`). We discard the outcome here to avoid
        // double-injecting (#106).
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

    // F1: pull the agent's `final_messages` out of the driver before the
    // borrow ends. The driver captures them from `TurnEvent::RunEnd`
    // regardless of `stopped_for`, so even a max-turns / cancelled run
    // still persists the user + partial-assistant turns it accumulated.
    let driver_final_messages = driver.take_final_messages();

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
            provider: provider_name(resolved_provider(args)),
            model: &model,
        };
        if let Err(e) = agent.hooks().session_end(&session_ctx, &outcome_ctx).await {
            tracing::warn!(target: caliban_common::tracing_targets::TARGET_HOOKS, error = %e, "session_end hook error (non-fatal)");
        }
        // The SessionEnd *hook* fires above (its observers/side-effects run), but
        // its stream-json `hook_event` frame is intentionally NOT flushed here:
        // `driver.run()` already emitted the terminal `result` frame, and a
        // post-result frame would violate ADR-0025's "last frame is `type:
        // result`" (#218). Earlier hook events (incl. SessionStart) are flushed
        // before the result via `emit_result`'s leading `flush_hook_events`.
    }

    // Save session back if requested.
    if let (Some(store), Some(mut s)) = (store.as_ref(), session)
        && !args.no_save
    {
        // F1: thread the driver's accumulated `final_messages` back into
        // the session so user/assistant turns from `-p --session NAME`
        // runs actually persist. Without this, the second `-p` against the
        // same session starts from a fresh transcript and the headline
        // `--session` flow in the README doesn't work via headless.
        //
        // Mirrors the single-prompt path's `s.merge_run(...)` (startup.rs
        // ~1202), but headless tracks token usage via `BudgetTracker`
        // rather than the agent-core `Usage` accumulator — we merge the
        // budget-tracked totals instead.
        if driver_final_messages.is_empty() {
            // No messages captured (e.g. run failed before the first
            // `RunEnd`). Still bump `updated_at` so the touch is observable.
            s.touch();
        } else {
            let (i_tok, o_tok) = budget.total_tokens();
            let run_usage = caliban_provider::Usage {
                input_tokens: u32::try_from(i_tok).unwrap_or(u32::MAX),
                output_tokens: u32::try_from(o_tok).unwrap_or(u32::MAX),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            };
            s.merge_run(driver_final_messages, run_usage);
        }
        if let Err(e) = persist_session(&mut s, store, &todos, &plan_mode) {
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
    match resolve_resume(args, store.as_ref(), &todos, &plan_mode) {
        Ok(Some(s)) => session = Some(s),
        Ok(None) => {}
        Err(e) => {
            eprintln!("[caliban] {e}");
            std::process::exit(headless::exit_code_for(&e));
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
        persist_session(s, store, &todos, &plan_mode)?;
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

#[cfg(test)]
mod tests {
    use super::{
        apply_system_and_schema, last_assistant_thinking_only, stop_condition_exit_code,
        stopped_for_surface_line,
    };
    use caliban_agent_core::StopCondition;
    use caliban_provider::{ContentBlock, Message, Role, TextBlock, ThinkingBlock};

    /// Concatenate the text blocks of a message (for asserting system content).
    fn system_text_of(m: &Message) -> String {
        m.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text(tb) => Some(tb.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn schema_directive_injected_when_no_system_message() {
        // Fresh run: no system message yet — base prompt + schema directive are
        // combined into a new leading System message.
        let mut messages: Vec<Message> = vec![];
        apply_system_and_schema(
            &mut messages,
            Some("BASE PROMPT".to_string()),
            Some("RESPOND WITH JSON ONLY".to_string()),
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::System);
        let t = system_text_of(&messages[0]);
        assert!(t.contains("BASE PROMPT"), "base prompt present: {t}");
        assert!(
            t.contains("RESPOND WITH JSON ONLY"),
            "schema directive present: {t}"
        );
    }

    #[test]
    fn schema_directive_appended_when_system_message_exists() {
        // Regression for #214: a sessioned/continued run already has a System
        // message (built in main.rs). --json-schema must still take effect —
        // the directive is appended rather than dropped.
        let mut messages = vec![Message::system_text("SESSION SYSTEM PROMPT")];
        apply_system_and_schema(
            &mut messages,
            Some("SESSION SYSTEM PROMPT".to_string()),
            Some("RESPOND WITH JSON ONLY".to_string()),
        );
        assert_eq!(messages.len(), 1, "no extra system message inserted");
        assert_eq!(messages[0].role, Role::System);
        let t = system_text_of(&messages[0]);
        assert!(
            t.contains("SESSION SYSTEM PROMPT"),
            "base prompt preserved: {t}"
        );
        assert!(
            t.contains("RESPOND WITH JSON ONLY"),
            "schema directive must apply even with an existing system message (#214); got: {t}"
        );
    }

    #[test]
    fn no_schema_leaves_existing_system_message_untouched() {
        // Without --json-schema, an existing system message is left as-is.
        let mut messages = vec![Message::system_text("SESSION SYSTEM PROMPT")];
        apply_system_and_schema(
            &mut messages,
            Some("SESSION SYSTEM PROMPT".to_string()),
            None,
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(system_text_of(&messages[0]), "SESSION SYSTEM PROMPT");
    }

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
    fn max_tokens_exhausted_surfaces_with_effort_low_hint() {
        let line = stopped_for_surface_line(&StopCondition::MaxTokensExhausted)
            .expect("max-tokens-exhausted must surface");
        assert!(line.contains("max-tokens recovery exhausted"));
        assert!(
            line.contains("/effort low"),
            "must hint at the one-keystroke remediation; got {line}"
        );
    }

    #[test]
    fn cancelled_surfaces() {
        let line =
            stopped_for_surface_line(&StopCondition::Cancelled).expect("cancellation must surface");
        assert!(line.contains("cancelled"));
    }

    // -- stop_condition_exit_code ----------------------------------------

    #[test]
    fn exit_code_end_of_turn_is_zero() {
        assert_eq!(stop_condition_exit_code(&StopCondition::EndOfTurn), 0);
    }

    #[test]
    fn exit_code_max_turns_is_seventyfive() {
        assert_eq!(
            stop_condition_exit_code(&StopCondition::MaxTurnsReached(50)),
            75
        );
    }

    #[test]
    fn exit_code_cancelled_is_onetwentyfour() {
        assert_eq!(stop_condition_exit_code(&StopCondition::Cancelled), 124);
    }

    #[test]
    fn exit_code_error_conditions_are_one() {
        for stop in [
            StopCondition::ProviderError("boom".into()),
            StopCondition::HookDenied("policy".into()),
            StopCondition::CompactionFailed("503".into()),
            StopCondition::MaxTokensExhausted,
        ] {
            assert_eq!(
                stop_condition_exit_code(&stop),
                1,
                "{stop:?} should map to exit 1"
            );
        }
    }
}
