//! Command-line argument parsing for the `caliban` binary.
//!
//! Hosts the `clap::Parser` [`Args`] struct, the [`CalibanCommand`]
//! subcommand tree, the [`ProviderKind`] value-enum and the small CLI
//! helpers (`read_prompt`, `summarize`, `default_model_for`,
//! `provider_name`) used by both the startup pipeline and the
//! subcommand handlers.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use anyhow::{Context, Result};
use caliban_provider::ContentBlock;
use clap::{Parser, ValueEnum};

use crate::headless;
use crate::router;

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

pub(crate) fn provider_name(p: ProviderKind) -> &'static str {
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

    /// Override the loopback port used by the OAuth callback server
    /// (ADR 0023 Phase C). Defaults to `0` (ephemeral); honors the
    /// `CALIBAN_MCP_OAUTH_PORT` env var when this flag is not set.
    #[arg(
        long = "mcp-oauth-port",
        value_name = "PORT",
        env = "CALIBAN_MCP_OAUTH_PORT"
    )]
    pub(crate) mcp_oauth_port: Option<u16>,

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

    /// Initial permission mode (ADR 0029). Valid values (camelCase):
    /// `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`,
    /// `bypassPermissions`. Overrides `CALIBAN_DEFAULT_PERMISSION_MODE`.
    #[arg(long = "permission-mode", value_name = "MODE")]
    pub(crate) permission_mode: Option<String>,

    /// DANGEROUS: required to enter `bypassPermissions` mode. Without this
    /// flag, the binary refuses to start in bypass mode and the
    /// Shift+Tab cycle skips past it (ADR 0029).
    #[arg(long = "allow-dangerously-skip-permissions")]
    pub(crate) allow_dangerously_skip_permissions: bool,

    /// Disable the auto-mode classifier. Every call that would be
    /// classified instead falls through to the Ask handler (ADR 0029).
    #[arg(long = "disable-auto-mode", env = "CALIBAN_DISABLE_AUTO_MODE")]
    pub(crate) disable_auto_mode: bool,

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

    /// Spawn a background sub-agent with the given task and return
    /// immediately. Equivalent to `caliban agents spawn --bg --prompt
    /// <task>`. ADR 0037.
    #[arg(long = "bg", value_name = "TASK")]
    pub(crate) bg: Option<String>,

    /// Inject a virtual settings scope above local (ADR 0026). Accepts
    /// inline JSON (`'{"model": "..."}'`) or a path to a `.json` /
    /// `.toml` file.
    #[arg(long = "settings", value_name = "FILE_OR_JSON")]
    pub(crate) settings_overlay: Option<String>,

    /// Restrict which `settings.json` scopes are read (CSV of
    /// `managed,user,project,local`). Useful for CI pinning a known-
    /// good base (ADR 0026).
    #[arg(long = "setting-sources", value_name = "CSV")]
    pub(crate) setting_sources: Option<String>,

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
    /// List, attach to, and manage background sub-agents (ADR 0037).
    Agents {
        #[command(subcommand)]
        inner: AgentsCommand,
    },
    /// Supervisor daemon management (ADR 0037).
    Daemon {
        #[command(subcommand)]
        inner: DaemonCommand,
    },
    /// Sugar for `caliban agents attach <id>`.
    Attach {
        /// Target agent id.
        id: String,
    },
    /// Sugar for `caliban agents logs <id>`.
    Logs {
        /// Target agent id.
        id: String,
    },
    /// Sugar for `caliban agents kill <id>`.
    Stop {
        /// Target agent id.
        id: String,
    },
    /// Sugar for `caliban agents kill <id>`.
    Kill {
        /// Target agent id.
        id: String,
    },
    /// Sugar for `caliban agents respawn <id>`.
    Respawn {
        /// Target agent id.
        id: String,
    },
    /// Sugar for `caliban agents rm <id>`.
    Rm {
        /// Target agent id.
        id: String,
        /// Force-remove even if the agent is still running.
        #[arg(long)]
        force: bool,
    },
}

/// `caliban agents <verb>` verbs.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum AgentsCommand {
    /// List registered background agents.
    List,
    /// Stream a running agent's transcript live (Ctrl+D detaches).
    Attach {
        /// Target agent id.
        id: String,
    },
    /// Print the agent's session log (`session.json`).
    Logs {
        /// Target agent id.
        id: String,
    },
    /// Terminate an agent (SIGTERM → SIGKILL after grace).
    Kill {
        /// Target agent id.
        id: String,
    },
    /// Restart an agent with the same spawn spec.
    Respawn {
        /// Target agent id.
        id: String,
    },
    /// Remove an agent from the registry (must be stopped or use `--force`).
    Rm {
        /// Target agent id.
        id: String,
        /// Force-remove even if the agent is still running.
        #[arg(long)]
        force: bool,
    },
    /// Spawn a new background agent.
    Spawn {
        /// Initial prompt for the new agent.
        #[arg(long)]
        prompt: String,
        /// Optional human-readable label.
        #[arg(long)]
        label: Option<String>,
    },
}

/// `caliban daemon <verb>` verbs.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum DaemonCommand {
    /// Print daemon health and the socket path.
    Status,
    /// Ask the daemon to shut down gracefully.
    Stop,
}

/// `caliban router <verb>` verbs.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum RouterCommand {
    /// Print the candidate list the router would resolve for a synthetic
    /// request, plus breaker state and effort knobs.
    Debug(router::RouterDebugArgs),
}

pub(crate) fn read_prompt(args: &Args) -> Result<String> {
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
