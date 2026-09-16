# Environment Variables

Caliban reads environment variables in two groups: `CALIBAN_*` variables that control the harness itself, and per-provider API-key and endpoint variables. Most `CALIBAN_*` flags mirror a corresponding CLI flag; the CLI flag always wins when both are set.

```admonish note title="Boolean flag variables"
Variables that mirror a boolean CLI flag (`CALIBAN_NO_MCP`, `CALIBAN_NO_HOOKS`,
`CALIBAN_AUTO_ALLOW`, and similar) use the flag parser: `1/true/yes/on` enable,
`0/false/no/off` disable (case-insensitive), and any other value is a startup error.
Where a row below says "any non-empty value", read it as "a truthy value".
```

---

## Provider API Keys

| Variable | Provider | Purpose |
|----------|----------|---------|
| `ANTHROPIC_API_KEY` | Anthropic | **Required.** API key for the Anthropic provider. |
| `ANTHROPIC_BASE_URL` | Anthropic | Optional. Override the Anthropic API base URL (useful for proxies or Bedrock-compatible endpoints). |
| `OPENAI_API_KEY` | OpenAI | **Required** for the hosted OpenAI API. Not required when `OPENAI_BASE_URL` points at a local OpenAI-compatible server. |
| `OPENAI_BASE_URL` | OpenAI | Optional. Override the OpenAI API base URL (for LM Studio, Mistral, and other OpenAI-compatible endpoints). |
| `OPENAI_ORG_ID` | OpenAI | Optional. OpenAI organization ID. |
| `OPENAI_PROJECT` | OpenAI | Optional. OpenAI project ID. |
| `AZURE_OPENAI_API_KEY` | Azure OpenAI | **Required** when using Azure OpenAI. |
| `AZURE_OPENAI_RESOURCE` | Azure OpenAI | **Required** when using Azure OpenAI. Azure resource name. |
| `AZURE_OPENAI_API_VERSION` | Azure OpenAI | Optional. API version string. Default: `2024-10-21`. |
| `GEMINI_API_KEY` | Google | **Required** when using the Google provider. `GOOGLE_GEMINI_API_KEY` is checked as a fallback. |
| `GOOGLE_GEMINI_API_KEY` | Google | Fallback for `GEMINI_API_KEY`. |

---

## Headless & Print Mode

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_MAX_ATTACH_BYTES` | `262144` (256 KB) | Maximum size of a single `@`-attachment. Also settable via `--max-attach-bytes`. |
| `CALIBAN_ATTACH_BUDGET_BYTES` | `1048576` (1 MB) | Aggregate size cap across all `@`-attachments in one message. Also settable via `--attach-budget-bytes`. |

---

## Permissions & Security

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_DEFAULT_PERMISSION_MODE` | `default` | Initial permission mode. Values: `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`. CLI `--permission-mode` wins when set. |
| `CALIBAN_NO_PERMISSIONS` | — | Any non-empty value disables permission gating (all tool calls allowed). Conflicts with `--allow`, `--deny`, `--ask`, `--auto-allow`. |
| `CALIBAN_AUTO_ALLOW` | — | **Dangerous.** Any non-empty value allows Ask-rule tools without prompting in non-interactive mode. |
| `CALIBAN_DISABLE_AUTO_MODE` | — | Any non-empty value disables the auto-mode classifier; all calls fall through to Ask. |

---

## Caching & Performance

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_NO_PROMPT_CACHE` | — | Any non-empty value disables Anthropic-style prompt caching. |
| `CALIBAN_NO_PARALLEL_TOOLS` | — | Any non-empty value forces serial tool execution. |
| `CALIBAN_PARALLEL_TOOL_LIMIT` | CPU cores − 1 (min 1) | Maximum concurrent tool invocations per turn. |

---

## Hooks, Skills, MCP & Plugins

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_NO_HOOKS` | — | Any non-empty value bypasses every external hook handler. In-process hooks still run. |
| `CALIBAN_NO_SKILLS` | — | Any non-empty value disables skill discovery at startup. |
| `CALIBAN_NO_MCP` | — | Any non-empty value disables MCP server discovery. |
| `CALIBAN_MCP_OAUTH_PORT` | `0` (ephemeral) | Loopback port for the MCP OAuth callback server (ADR 0023 Phase C). |
| `CALIBAN_MCP_TIMEOUT` | — | Timeout (ms) for MCP server startup/connection. |
| `CALIBAN_MCP_TOOL_TIMEOUT` | — | Per-tool-call timeout (ms) for MCP tools. |
| `CALIBAN_NO_PLUGINS` | — | Any non-empty value disables plugin discovery. |
| `CALIBAN_ENABLED_PLUGINS` | — | Comma-separated list of plugin names to enable (all others disabled). |
| `CALIBAN_PLUGIN_ROOT` | — | Override the plugin install root directory. |

---

## Sub-agents

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_NO_SUB_AGENT` | — | Any non-empty value disables the built-in `AgentTool`. |
| `CALIBAN_DAEMON_RUNTIME_DIR` | Platform default | Override the runtime socket directory for the supervisor daemon. |
| `CALIBAN_DAEMON_LISTEN` | — | TCP listen address (e.g. `0.0.0.0:7000`) that switches the `caliband` supervisor into networked control-plane mode; the `caliban agents` CLI dials the same address to reach a remote daemon. Unset means the local Unix-socket path. TLS/token come from the `CALIBAN_DAEMON_TLS_*` / `CALIBAN_DAEMON_TOKEN` vars. |
| `CALIBAN_KEEP_WORKTREES` | — | Debug escape hatch: keep sub-agent worktrees instead of removing them when the worker exits. |

---

## Driving (serve surfaces)

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_DRIVE_TOKEN` | — | Bearer token that non-loopback peers must present to `caliban http serve` (and the other drive surfaces). Binding to a non-loopback address without it is refused. See [Driving Caliban](../driving/overview.md). |

---

## Memory

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_DISABLE_AUTO_MEMORY` | — | Any non-empty value disables auto-memory topic-file writing. |
| `CALIBAN_MEMORY_DIR` | Platform default | Override the auto-memory topic files directory. |
| `CALIBAN_MEMORY_BUDGET_TOKENS` | — | Total token budget across all memory tiers. |
| `CALIBAN_MEMORY_CAP_TOKENS_AUTO` | — | Token budget cap for the auto-memory tier. |
| `CALIBAN_MEMORY_CAP_TOKENS_CLAUDE_MD` | — | Token budget cap for the CLAUDE.md tier. |
| `CALIBAN_AUTO_MEMORY_DIRECTORY` | — | Override the auto-memory directory (alias form). |
| `CALIBAN_DISABLE_CLAUDE_MD_WALK` | — | Any non-empty value disables the CLAUDE.md walk-up discovery. |
| `CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD` | — | Colon-separated list of extra directories to search for CLAUDE.md. |
| `CALIBAN_CLAUDE_MD_EXCLUDES` | — | Colon-separated glob patterns to exclude from CLAUDE.md discovery. |
| `CALIBAN_APPROVE_IMPORTS` | — | Any non-empty value auto-approves CLAUDE.md `@import` statements. |

---

## Storage (remote memory substrate)

These override the `[storage]` settings so caliban can be pointed at a remote
gonzalo daemon **without** a settings file (env wins over the file; a blank value
is ignored, leaving the file setting in place). Substrate values use the same
vocabulary as the settings file — `fs` / `remote` / `git` / `s3`, i.e. `fs`, not
`local` — and an invalid value is a startup error naming the variable. The bearer
token itself is never set here: `CALIBAN_STORAGE_REMOTE_TOKEN_ENV` names the
*variable* that holds it, so no secret lives in settings or in these overrides.

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_STORAGE_SUBSTRATE` | `fs` | Override `storage.substrate` — `fs` (local filesystem) or `remote` (a gonzalo daemon); `git`/`s3` are recognized but not wired yet. |
| `CALIBAN_STORAGE_REMOTE_URL` | — | Override `storage.remote.url` — the gonzalod base URL (e.g. `http://host:8080`). Creates the `[storage.remote]` block if absent. |
| `CALIBAN_STORAGE_REMOTE_TOKEN_ENV` | — | Override `storage.remote.token_env` — the *name* of the env var holding the gonzalod bearer token (e.g. `GONZALO_TOKEN`), not the token itself. |

---

## Checkpoints

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_CHECKPOINT_ROOT` | `~/.local/share/caliban/projects` | Override the checkpoint root directory. |
| `CALIBAN_CHECKPOINT_DISABLED` | — | Any non-empty value disables checkpoint recording and pruning. |
| `CALIBAN_CHECKPOINT_MAX_FILE_BYTES` | — | Maximum checkpoint file size before rotation. |
| `CALIBAN_CLEANUP_PERIOD_DAYS` | — | Number of days after which old checkpoint files are pruned. |

---

## Configuration & Router

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_ROUTER_CONFIG` | Walk-up discovery | Explicit path to `caliban.toml`. Also settable via `--config`. |
| `CALIBAN_STRICT_ROUTING` | — | Any non-empty value enables strict routing (no fallback to default route on unknown purpose). |
| `CALIBAN_API_KEY_HELPER_TTL_MS` | — | TTL in milliseconds for API key helper subprocess cache. |

---

## Web Search

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_WEBSEARCH_PROVIDER` | `brave` | Web-search backend for the `WebSearch` tool. Values: `brave`, `tavily`, `exa`. |
| `BRAVE_API_KEY` | — | API key for the Brave search provider. |
| `TAVILY_API_KEY` | — | API key for the Tavily search provider. |
| `EXA_API_KEY` | — | API key for the Exa search provider. |

---

## Output

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_OUTPUT_STYLE` | — | Name of the active output style (see [Output Styles](../extending/output-styles.md)). |
| `CALIBAN_GRAPHICS` | — | Graphics capability hint (e.g. `kitty`, `sixel`). |

---

## Observability & Telemetry

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_ENABLE_TELEMETRY` | — | Any non-empty value enables OTel telemetry (settings `enable_telemetry` is also checked). |
| `CALIBAN_OTEL_HEADERS_HELPER` | — | Command to supply dynamic OTel export headers. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | — | OTel OTLP exporter endpoint URL. |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | OTel OTLP transport protocol. |
| `OTEL_EXPORTER_OTLP_HEADERS` | — | Additional headers for the OTLP exporter. |
| `OTEL_METRIC_EXPORT_INTERVAL` | `60s` | OTel metric export interval. |
| `OTEL_LOG_USER_PROMPTS` | `false` | Opt-in capture of prompt/completion content on `gen_ai` spans. Off by default; any truthy value records user prompts and model completions as span content (ADR 0053). |
| `OTEL_LOGS_EXPORTER` | `otlp` | OTel logs exporter type. |
| `OTEL_METRICS_EXPORTER` | `otlp` | OTel metrics exporter type. |
| `OTEL_TRACES_EXPORTER` | `otlp` | OTel traces exporter type. |
| `CALIBAN_RATES_YAML` | — | Path to a YAML file overriding the built-in provider pricing rate card. |

---

## Debug

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_DEBUG` | — | Any non-empty value enables the file-backed tracing subscriber (appends to the platform debug log). Also settable via `--debug`. |
| `CALIBAN_DEBUG_FILE` | — | Redirect debug output to this path (implies debug). Also settable via `--debug-file`. |

---

## Plugin Trust & Marketplace

| Variable | Default | Description |
|----------|---------|-------------|
| `CALIBAN_BLOCKED_MARKETPLACES` | — | Comma-separated list of marketplace names to block. |
| `CALIBAN_STRICT_KNOWN_MARKETPLACES` | — | Any non-empty value blocks installs from unrecognized marketplaces. |
| `CALIBAN_STRICT_PLUGIN_ONLY_CUSTOMIZATION` | — | Any non-empty value restricts customization to plugins only (no user-level skills/hooks). |

---

```admonish note title="CALIBAN_PROVIDER is an output, not an input"
Caliban does not read `CALIBAN_PROVIDER` to choose a provider. Use `--provider` or settings for that. Caliban
**sets** `CALIBAN_PROVIDER` (and `CALIBAN_API_KEY_HELPER_TTL_MS`) in the environment of an
`api_key_helper` process so the script knows which provider's key to print. See
[Configuring Providers & API Keys](../providers/configuration.md).
```
