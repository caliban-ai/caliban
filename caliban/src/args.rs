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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ProviderKind {
    Anthropic,
    Openai,
    Ollama,
    Google,
}

pub(crate) fn default_model_for(p: ProviderKind) -> &'static str {
    match p {
        ProviderKind::Anthropic => "claude-sonnet-4-6",
        ProviderKind::Openai => "gpt-5.5",
        ProviderKind::Ollama => "llama3.1",
        ProviderKind::Google => "gemini-2.0-flash",
    }
}

/// Unwrap the CLI-resolved provider. `main.rs` writes
/// `args.provider = Some(effective.provider)` after `EffectiveModel`
/// resolution, so this accessor returns the precedence-resolved
/// provider in all normal call paths. The `unwrap_or(Anthropic)` is a
/// safety net for code paths that read `args.provider` before main has
/// run resolution (e.g. early-startup error reporting).
#[must_use]
pub(crate) fn resolved_provider(args: &Args) -> ProviderKind {
    args.provider.unwrap_or(ProviderKind::Anthropic)
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
    #[arg(short = 'p', long = "print", value_name = "PROMPT", num_args = 0..=1, default_missing_value = "", help_heading = "Headless / -p mode (ADR 0025)")]
    pub(crate) print: Option<String>,

    /// Stream-output format (headless mode only).
    #[arg(
        long = "output-format",
        value_enum,
        value_name = "FMT",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) output_format: Option<headless::OutputFormat>,

    /// Suppress the ADR 0025 auto-headless dispatch when stdout is
    /// piped or stdin is non-TTY. Explicit `--print` / `--output-format`
    /// always wins; this flag only governs the implicit fall-through.
    #[arg(long = "no-auto-print", help_heading = "Headless / -p mode (ADR 0025)")]
    pub(crate) no_auto_print: bool,

    /// Stdin format (headless mode only).
    #[arg(
        long = "input-format",
        value_enum,
        value_name = "FMT",
        default_value = "text",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) input_format: headless::InputFormat,

    /// Maximum cumulative cost in USD before the run aborts (exit 137).
    /// Cost is computed against the rate card in `caliban-telemetry::pricing`;
    /// unknown `(provider, model)` pairs contribute 0.0 and emit a warning.
    #[arg(
        long = "max-budget-usd",
        value_name = "USD",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) max_budget_usd: Option<f64>,

    /// Skip hooks/skills/plugins/MCP/auto-memory/CLAUDE.md discovery
    /// (deterministic CI mode; ADR 0025).
    #[arg(long = "bare", help_heading = "Headless / -p mode (ADR 0025)")]
    pub(crate) bare: bool,

    /// Force structured final output matching the given JSON Schema. Value
    /// can be inline JSON or a path to a `.json` file.
    #[arg(
        long = "json-schema",
        value_name = "FILE_OR_JSON",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) json_schema: Option<String>,

    /// Emit assistant text deltas as separate `text` frames in
    /// stream-json mode (default: aggregate into one `message` frame).
    #[arg(
        long = "include-partial-messages",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) include_partial_messages: bool,

    /// Emit a `hook_event` frame per fired hook event in stream-json mode.
    #[arg(
        long = "include-hook-events",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) include_hook_events: bool,

    /// Echo each user prompt as a `user` frame in stream-json mode.
    #[arg(
        long = "replay-user-messages",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) replay_user_messages: bool,

    /// Resume the most recently updated session.
    #[arg(short = 'c', long = "continue")]
    pub(crate) continue_latest: bool,

    /// Resume a named session.
    #[arg(short = 'r', long = "resume", value_name = "NAME")]
    pub(crate) resume: Option<String>,

    /// Fallback model to use when the primary model errors. Wired
    /// end-to-end through `caliban-model-router` (ADR 0038); also
    /// surfaced in the headless `system/init` frame.
    #[arg(long = "fallback-model", value_name = "MODEL")]
    pub(crate) fallback_model: Option<String>,

    /// Route permission `Ask` events to the named MCP tool via the MCP
    /// elicitation channel (ADR 0023 Phase C).
    #[arg(long = "permission-prompt-tool", value_name = "MCP_TOOL")]
    pub(crate) permission_prompt_tool: Option<String>,

    /// Which provider to use. If omitted, resolved from `Settings.model`
    /// (project/local/user/managed scope); falls back to Anthropic when
    /// neither CLI nor Settings supply one.
    #[arg(long, value_enum)]
    pub(crate) provider: Option<ProviderKind>,

    /// Model name (defaults per provider)
    #[arg(long)]
    pub(crate) model: Option<String>,

    /// Per-turn output token limit (must be ≥ 1).
    //
    // 8192 (not the older 2048) gives verbose reasoning models enough room to
    // finish a turn — thinking + tool call — without tripping the `MaxTokens`
    // recovery escalation on nearly every substantial turn. Half the
    // `escalated_max_tokens` ceiling (16384), so Stage A still has headroom.
    #[arg(long, default_value_t = 8192, value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) max_tokens: u32,

    /// Maximum agent loop iterations
    #[arg(long, default_value_t = 50)]
    pub(crate) max_turns: u32,

    /// Enable Stage A budget escalation + Stage B meta-continuation when a
    /// turn ends in `MaxTokens` (the "max-tokens recovery" two-stage flow).
    /// Default is `true`; pass `--max-tokens-recovery=false` to opt out.
    ///
    /// Precedence: CLI flag > settings `max_tokens_recovery` > built-in
    /// default. Honored by `caliban-agent-core::stream::Agent`.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    pub(crate) max_tokens_recovery: Option<bool>,

    /// Sampling temperature in `[0.0, 2.0]`. Above 2.0 is rejected
    /// rather than silently clamped — providers disagree on the
    /// max-acceptable value, and passing a bad temperature through
    /// would surface as an opaque mid-stream provider error.
    #[arg(long, value_parser = parse_temperature, allow_negative_numbers = true)]
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
    /// (`~/Library/Caches/caliban/debug.log` on macOS). `CALIBAN_DEBUG`
    /// is also honored — any non-empty value turns it on.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub(crate) debug: bool,

    /// Redirect debug output to this path instead of the default
    /// `~/.cache/caliban/debug.log`. Implies `--debug` (naming a destination
    /// turns logging on). Relative paths resolve against the current
    /// directory. `CALIBAN_DEBUG_FILE` is also honored.
    #[arg(long, value_name = "PATH", env = "CALIBAN_DEBUG_FILE")]
    pub(crate) debug_file: Option<PathBuf>,

    /// Dump full, untruncated tool inputs/outputs to stderr in headless
    /// `--output-format text`. No effect on `stream-json` (already emits full
    /// `tool_use`/`tool_result` frames) or `json`. `CALIBAN_VERBOSE` is also
    /// honored.
    #[arg(
        long,
        env = "CALIBAN_VERBOSE",
        help_heading = "Headless / -p mode (ADR 0025)"
    )]
    pub(crate) verbose: bool,

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

    /// Disable MCP server discovery (skip the unified `settings.json`
    /// `mcp.servers` block and the legacy `mcp.toml` compat shim).
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

    /// Bypass every external hook handler (debugging escape hatch).
    /// Mirrors the `hooks.disable_all_hooks` field in `settings.json`
    /// but applies one-off. In-process hooks (`PermissionsHook`, audit)
    /// still run.
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
    /// Run health checks against the local caliban install (settings,
    /// MCP, sandbox, stores, providers).
    Doctor {
        /// Include deep checks (provider auth pings — costs an API call
        /// per configured provider).
        #[arg(long)]
        deep: bool,
    },
    /// Inspect / migrate settings (ADR 0026).
    Config {
        #[command(subcommand)]
        inner: ConfigCommand,
    },
    /// Manage plugin packages (ADR 0030).
    ///
    /// Verbs: `list`, `info <name>`, `install <name>@<marketplace>
    /// [--yes]`, `install --dir <path>`, `update <name> [--yes]`,
    /// `remove <name>`, `enable <name>`, `disable <name>`.
    /// Run `caliban plugin help` for the full reference.
    #[command(trailing_var_arg = true, allow_hyphen_values = true)]
    Plugin {
        /// Plugin sub-verb plus its arguments. The plugin CLI parses
        /// these directly (mirrors how `cargo` forwards positional
        /// args to subcommands).
        #[arg(value_name = "ARGS")]
        args: Vec<String>,
    },
    /// Manage permission rules across all config scopes.
    Perms {
        #[command(subcommand)]
        cmd: PermsCommand,
    },
    /// Manage caliban-wide settings (import, print).
    Settings {
        #[command(subcommand)]
        cmd: SettingsCommand,
    },
}

/// `caliban perms <verb>` verbs.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum PermsCommand {
    /// List permission rules (all scopes merged, or a specific scope).
    List {
        /// Restrict output to a single scope (managed/user/project/local/cli).
        #[arg(long)]
        scope: Option<String>,
        /// Show the effective merged rule list across all scopes.
        #[arg(long)]
        effective: bool,
        /// Emit JSON instead of the default human-readable table.
        #[arg(long)]
        json: bool,
    },
    /// Test whether a tool call would be allowed, denied, or asked.
    Test {
        /// Tool name (e.g. `Bash`).
        tool: String,
        /// Tool input as JSON (e.g. `'{"command":"git push"}'`).
        #[arg(value_parser = parse_input_json)]
        input: Option<serde_json::Value>,
    },
    /// Show which rule in the list first matches a tool call.
    Explain {
        /// Tool name (e.g. `Bash`).
        tool: String,
        /// Tool input as JSON (e.g. `'{"command":"git push"}'`).
        #[arg(value_parser = parse_input_json)]
        input: Option<serde_json::Value>,
    },
    /// Add a permission rule to a scope file.
    Add {
        /// Pattern of the form `Tool` or `Tool:first-arg-glob`.
        pattern: String,
        /// Action: `allow`, `ask`, or `deny`.
        action: String,
        /// Target scope (default: `project`).
        #[arg(long)]
        scope: Option<String>,
        /// Optional human-readable comment stored in the rule.
        #[arg(long)]
        comment: Option<String>,
        /// Optional deny reason shown to the operator.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove a permission rule from a scope file.
    Remove {
        /// Remove by ordinal position (1-based).
        #[arg(long)]
        index: Option<usize>,
        /// Remove all rules whose pattern equals this value.
        #[arg(long)]
        pattern: Option<String>,
        /// Target scope (default: `project`).
        #[arg(long)]
        scope: Option<String>,
    },
    /// Import rules from a foreign config (Claude Code JSON, legacy caliban TOML).
    Import {
        /// Path to the source file.
        #[arg(long, value_name = "PATH")]
        from: std::path::PathBuf,
        /// Destination scope (default: `user`).
        #[arg(long)]
        scope: Option<String>,
        /// Print what would be imported without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Export permission rules to stdout in TOML or JSON format.
    Export {
        /// Source scope (default: `project`).
        #[arg(long)]
        scope: Option<String>,
        /// Output format: `toml` (default) or `json`.
        #[arg(long, default_value = "toml")]
        format: String,
    },
    /// Show the permission-decision audit log (full impl in Phase 7).
    Audit {
        /// Show decisions since this ISO timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Filter by tool name.
        #[arg(long)]
        tool: Option<String>,
        /// Filter by action (allow/ask/deny).
        #[arg(long)]
        action: Option<String>,
        /// Limit output to the most-recent N entries.
        #[arg(long)]
        head: Option<usize>,
    },
    /// Check for duplicate or conflicting rules in a scope file.
    Lint {
        /// Scope to lint (default: `project`).
        #[arg(long)]
        scope: Option<String>,
    },
}

fn parse_input_json(s: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(s).map_err(|e| e.to_string())
}

/// `caliban settings <verb>` verbs.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum SettingsCommand {
    /// Import a settings JSON (Claude Code / Codex / legacy caliban) into
    /// canonical caliban TOML.
    Import {
        /// Path to the source file.
        #[arg(long, value_name = "PATH")]
        from: std::path::PathBuf,
        /// Destination scope (default: `project`).
        #[arg(long)]
        scope: Option<String>,
        /// Print what would be imported without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the settings for a scope (or the merged effective settings).
    Print {
        /// Scope to print (default: `project`).
        #[arg(long)]
        scope: Option<String>,
    },
}

/// `caliban config <verb>` verbs.
#[derive(Debug, Clone, clap::Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print the merged effective settings as JSON, including the per-
    /// key scope chain. Honors `--settings` / `--setting-sources`.
    Print,
    /// Round-trip legacy per-feature TOMLs (`permissions.toml`,
    /// `mcp.toml`, `hooks.toml`) under the workspace into a single
    /// project-scope `settings.json`. Writes to
    /// `<workspace>/.caliban/settings.json`. Existing keys in that file
    /// are preserved; the migrated keys are merged on top.
    Migrate {
        /// Print the migration result to stdout without writing.
        #[arg(long)]
        dry_run: bool,
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

/// clap `value_parser` for `--temperature`. Validates the input is a
/// finite `f32` in `[0.0, 2.0]`.
fn parse_temperature(s: &str) -> Result<f32, String> {
    let n: f32 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if !n.is_finite() {
        return Err(format!("`{s}` is not finite"));
    }
    if !(0.0..=2.0).contains(&n) {
        return Err(format!(
            "temperature `{s}` is outside [0.0, 2.0]; pass a value the provider supports"
        ));
    }
    Ok(n)
}

/// Post-parse validation for combinations clap can't express natively.
///
/// `--input-format stream-json` consumes stdin as a chat transcript;
/// any inline prompt would silently bypass the NDJSON parser. Reject
/// inline prompts at startup with `EX_USAGE`-style messaging so
/// operators see the conflict immediately instead of debugging a
/// blank-prompt agent run (lmstudio Finding 13).
///
/// The `-` sentinel is allowed in any prompt slot because it
/// explicitly delegates to stdin — semantically identical to omitting
/// the flag in stream-json mode.
///
/// # Errors
/// Returns an `EX_USAGE`-style anyhow error when a non-`-` prompt is
/// combined with `--input-format stream-json`.
pub(crate) fn validate_stream_json_input(args: &Args) -> Result<()> {
    if !matches!(args.input_format, headless::InputFormat::StreamJson) {
        return Ok(());
    }
    let slots = [
        ("--print / -p", args.print.as_deref()),
        ("--prompt", args.prompt_flag.as_deref()),
        ("PROMPT (positional)", args.prompt.as_deref()),
    ];
    for (slot, val) in slots {
        if let Some(v) = val
            && v != "-"
            && !v.is_empty()
        {
            anyhow::bail!(
                "`{slot}` is incompatible with `--input-format stream-json`: stdin is the NDJSON \
                 frame stream. Pass `-` (or omit the prompt flag entirely) to read frames from stdin."
            );
        }
    }
    Ok(())
}

pub(crate) fn read_prompt(args: &Args) -> Result<String> {
    use std::io::Read as _;
    let raw = args
        .prompt_flag
        .as_deref()
        .or(args.prompt.as_deref())
        .context("no prompt given (use positional argument or --prompt)")?;
    let text = if raw == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        raw.to_string()
    };
    if text.trim().is_empty() {
        anyhow::bail!(
            "empty prompt — pass non-empty text via positional argument, --prompt, or stdin"
        );
    }
    Ok(text)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `Args` value with the listed extra CLI args appended.
    /// Avoids stomping on test process state by always passing the
    /// `caliban` binary name as `argv[0]`.
    fn parse(extra: &[&str]) -> Args {
        let mut argv: Vec<&str> = vec!["caliban"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).expect("clap parse")
    }

    #[test]
    fn validate_stream_json_input_allows_dash_print() {
        // `-p -` explicitly delegates to stdin; valid under stream-json input.
        let args = parse(&["--input-format", "stream-json", "-p", "-"]);
        assert!(validate_stream_json_input(&args).is_ok());
    }

    #[test]
    fn validate_stream_json_input_allows_no_prompt_at_all() {
        // Omitting the prompt is the canonical stream-json invocation.
        let args = parse(&["--input-format", "stream-json"]);
        assert!(validate_stream_json_input(&args).is_ok());
    }

    #[test]
    fn validate_stream_json_input_rejects_inline_print_prompt() {
        // Inline `-p "hi"` would silently bypass the NDJSON parser
        // (lmstudio Finding 13). Must fail loud.
        let args = parse(&["--input-format", "stream-json", "-p", "hi"]);
        let err = validate_stream_json_input(&args).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("stream-json"),
            "error must name stream-json conflict; got {msg}"
        );
    }

    #[test]
    fn validate_stream_json_input_rejects_inline_positional_prompt() {
        let args = parse(&["--input-format", "stream-json", "hello"]);
        assert!(validate_stream_json_input(&args).is_err());
    }

    #[test]
    fn validate_stream_json_input_rejects_inline_prompt_flag() {
        let args = parse(&["--input-format", "stream-json", "--prompt", "hello"]);
        assert!(validate_stream_json_input(&args).is_err());
    }

    #[test]
    fn validate_stream_json_input_noop_in_text_mode() {
        // Inline prompts are obviously fine in the default text mode.
        let args = parse(&["-p", "hello"]);
        assert!(validate_stream_json_input(&args).is_ok());
    }

    #[test]
    fn debug_file_flag_parses_to_path() {
        let args = parse(&["--debug-file", "/var/log/caliban.log"]);
        assert_eq!(
            args.debug_file,
            Some(std::path::PathBuf::from("/var/log/caliban.log"))
        );
    }

    #[test]
    fn debug_file_absent_by_default() {
        // Guard on the env var so an exported CALIBAN_DEBUG_FILE doesn't flake.
        if std::env::var_os("CALIBAN_DEBUG_FILE").is_none() {
            assert!(parse(&[]).debug_file.is_none());
        }
    }

    #[test]
    fn verbose_flag_parses() {
        assert!(parse(&["--verbose"]).verbose);
    }

    #[test]
    fn verbose_absent_by_default() {
        // Guard on the env var so an exported CALIBAN_VERBOSE doesn't flake.
        if std::env::var_os("CALIBAN_VERBOSE").is_none() {
            assert!(!parse(&[]).verbose);
        }
    }

    // -- pure CLI helpers -------------------------------------------------

    #[test]
    fn read_prompt_prefers_flag_over_positional() {
        let args = parse(&["positional-prompt", "--prompt", "flag-prompt"]);
        assert_eq!(read_prompt(&args).unwrap(), "flag-prompt");
    }

    #[test]
    fn read_prompt_falls_back_to_positional() {
        let args = parse(&["positional-prompt"]);
        assert_eq!(read_prompt(&args).unwrap(), "positional-prompt");
    }

    #[test]
    fn read_prompt_errors_when_missing() {
        let args = parse(&[]);
        assert!(read_prompt(&args).is_err());
    }

    #[test]
    fn read_prompt_errors_on_whitespace_only() {
        let args = parse(&["--prompt", "   "]);
        let err = read_prompt(&args).unwrap_err().to_string();
        assert!(err.contains("empty prompt"), "got: {err}");
    }

    #[test]
    fn summarize_passes_through_short_single_line() {
        assert_eq!(summarize("hello", 80), "hello");
    }

    #[test]
    fn summarize_truncates_long_line_with_ellipsis() {
        let out = summarize("abcdefghij", 4);
        assert_eq!(out, "abcd\u{2026}");
    }

    #[test]
    fn summarize_marks_multiline_with_ellipsis() {
        let out = summarize("line one\nline two", 80);
        assert_eq!(out, "line one\u{2026}");
    }

    #[test]
    fn summarize_blocks_returns_first_text_block() {
        let blocks = vec![
            ContentBlock::Text(caliban_provider::TextBlock {
                text: "first line\nsecond line".into(),
                cache_control: None,
            }),
            ContentBlock::Text(caliban_provider::TextBlock {
                text: "ignored".into(),
                cache_control: None,
            }),
        ];
        // First text block wins, and multiline summarization applies.
        assert_eq!(summarize_blocks(&blocks, 80), "first line\u{2026}");
    }

    #[test]
    fn summarize_blocks_handles_no_text() {
        assert_eq!(summarize_blocks(&[], 80), "(no text)");
    }

    #[test]
    fn parse_temperature_accepts_boundaries() {
        assert!(parse_temperature("0.0").unwrap().abs() < f32::EPSILON);
        assert!((parse_temperature("2.0").unwrap() - 2.0).abs() < f32::EPSILON);
        assert!((parse_temperature("0.7").unwrap() - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_temperature_rejects_out_of_range_and_garbage() {
        assert!(parse_temperature("2.5").is_err());
        assert!(parse_temperature("-0.1").is_err());
        assert!(parse_temperature("NaN").is_err());
        assert!(parse_temperature("inf").is_err());
        assert!(parse_temperature("not-a-number").is_err());
    }

    #[test]
    fn default_model_for_covers_every_provider() {
        assert_eq!(
            default_model_for(ProviderKind::Anthropic),
            "claude-sonnet-4-6"
        );
        assert_eq!(default_model_for(ProviderKind::Openai), "gpt-5.5");
        assert_eq!(default_model_for(ProviderKind::Ollama), "llama3.1");
        assert_eq!(default_model_for(ProviderKind::Google), "gemini-2.0-flash");
    }

    #[test]
    fn provider_name_covers_every_provider() {
        assert_eq!(provider_name(ProviderKind::Anthropic), "anthropic");
        assert_eq!(provider_name(ProviderKind::Openai), "openai");
        assert_eq!(provider_name(ProviderKind::Ollama), "ollama");
        assert_eq!(provider_name(ProviderKind::Google), "google");
    }

    #[test]
    fn resolved_provider_defaults_to_anthropic() {
        assert_eq!(resolved_provider(&parse(&[])), ProviderKind::Anthropic);
    }

    #[test]
    fn resolved_provider_reflects_explicit_flag() {
        let args = parse(&["--provider", "openai"]);
        assert_eq!(resolved_provider(&args), ProviderKind::Openai);
    }
}
