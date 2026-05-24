# Claude Code documented-capability inventory

> Structured snapshot of Claude Code's documented surface, captured from
> the public docs at `docs.claude.com/en/docs/claude-code/*` on
> **2026-05-24**. This is the source feeding
> [`parity-gap-matrix.md`](parity-gap-matrix.md); refresh both together
> when Claude Code ships new features.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines
> name the canonical configuration mechanism.

## 1. Overview / "Use Claude Code everywhere"

- **What it does:** Positions Claude Code as a single agentic engine accessible from multiple surfaces. Each surface shares CLAUDE.md, settings, MCP servers.
- **Key surfaces:** Terminal CLI (`claude`), VS Code / Cursor / forks extension, JetBrains plugin, Desktop app (macOS/Win/Linux), Web (`claude.ai/code`), iOS app, Slack, GitHub Actions, GitLab CI/CD, Chrome extension, Remote Control, Routines, Dispatch, Channels, Background agents (`--bg`, `claude agents`).
- **Built-in tools listed:** not enumerated on this page; referenced elsewhere (Bash, file edits, MCP, Skill).
- **Install methods:** native installer (`curl https://claude.ai/install.sh | bash`), Homebrew (`claude-code` / `claude-code@latest`), WinGet (`Anthropic.ClaudeCode`), apt/dnf/apk.
- **Config:** install scripts; per-surface install; auth via `claude` first run.

## 2. Quickstart

- **What it does:** Tutorial walking from install → `/login` → first session → first edit → git workflow → bug fix → tests/refactor.
- **Key surfaces:** `claude`, `claude "task"`, `claude -p`, `claude -c`, `claude -r`, `/clear`, `/help`, `/login`, `/resume`, `exit`/Ctrl+D, Tab completion, ↑ history, Shift+Tab to cycle permission modes.
- **Config:** relies on CLI commands and `/config` later.

## 3. CLI reference

- **What it does:** Authoritative list of `claude` subcommands and flags. Notes that `--help` is incomplete.
- **Subcommands:** `claude`, `claude "query"`, `claude -p "query"`, `claude -c`, `claude -r "<session>"`, `claude update`, `claude install [version]`, `claude auth login|logout|status`, `claude agents`, `claude attach <id>`, `claude auto-mode defaults|config`, `claude daemon status`, `claude logs <id>`, `claude mcp ...`, `claude plugin ...` (alias `plugins`), `claude project purge`, `claude remote-control`, `claude respawn <id>`, `claude rm <id>`, `claude setup-token`, `claude stop|kill <id>`, `claude ultrareview`.
- **Flags:** `--add-dir`, `--agent`, `--agents <json>`, `--allow-dangerously-skip-permissions`, `--allowedTools`, `--append-system-prompt[-file]`, `--bare`, `--betas`, `--bg`, `--channels`, `--chrome`, `--continue`/`-c`, `--dangerously-skip-permissions`, `--debug[-file]`, `--disable-slash-commands`, `--disallowedTools`, `--effort`, `--exclude-dynamic-system-prompt-sections`, `--fallback-model`, `--fork-session`, `--from-pr`, `--ide`, `--init` / `--init-only` / `--maintenance`, `--include-hook-events`, `--include-partial-messages`, `--input-format`, `--json-schema`, `--max-budget-usd`, `--max-turns`, `--mcp-config`, `--model`, `--name`/`-n`, `--no-session-persistence`, `--output-format` (`text|json|stream-json`), `--permission-mode` (`default|acceptEdits|plan|auto|dontAsk|bypassPermissions`), `--permission-prompt-tool`, `--plugin-dir`, `--plugin-url`, `--print`/`-p`, `--remote`, `--remote-control`/`--rc`, `--replay-user-messages`, `--resume`/`-r`, `--session-id`, `--setting-sources` (`user,project,local`), `--settings` (file or inline JSON), `--strict-mcp-config`, `--system-prompt[-file]`, `--teleport`, `--teammate-mode` (`auto|in-process|tmux`), `--tmux`, `--tools` (`""`/`default`/csv), `--verbose`, `--version`/`-v`, `--worktree`/`-w`.

## 4. Interactive mode

- **What it does:** TUI keybindings, vim mode, transcript viewer, background bash, prompt suggestions, side questions, task list, session recap, PR badge.
- **Key bindings:** Ctrl+C (interrupt/exit), Ctrl+X Ctrl+K (kill background subagents), Ctrl+D, Ctrl+G/Ctrl+X Ctrl+E (external editor), Ctrl+L (redraw), Ctrl+O (transcript), Ctrl+R (history search), Ctrl+B (background), Ctrl+T (task list/syntax-highlight in theme picker), Esc (interrupt), Esc-Esc (clear/rewind), Shift+Tab (permission-mode cycle), Option+P (switch model), Option+T (extended thinking), Option+O (fast mode); text-editing readline bindings (Ctrl+A/E/K/U/W/Y, Alt+B/F, Alt+Y paste cycle); multiline (`\`+Enter, Option+Enter, Shift+Enter native in iTerm2/WezTerm/Ghostty/Kitty/Warp/Apple Terminal/Win Term, Ctrl+J fallback); `/` (commands), `!` (shell mode), `@` (file autocomplete); transcript viewer keys `?`, `{`/`}`, Ctrl+E (toggle show all), `[` (dump to scrollback), `v` (open in $VISUAL), `q`/Esc; voice dictation Space (hold/tap, configurable).
- **Vim mode:** full normal/insert/visual modes with motions and text objects; block-visual (Ctrl+V) not supported.
- **Other features:** command history per-cwd; reverse search scoped to session/project/all-projects via Ctrl+S; background bash with 5 GB cap, can be disabled via `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`; prompt suggestions from git history, disable with `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false`; `/btw` side questions (no tools, ephemeral); task list with `CLAUDE_CODE_TASK_LIST_ID` for named persistence; session recap (`/recap`); PR badge polled every 60 s, requires `gh` CLI.

## 5. Slash commands / Skills (merged page)

- **What it does:** Skills replace the legacy `.claude/commands/`. SKILL.md is a YAML-front-mattered markdown file Claude can invoke via `/name` or autonomously when description matches.
- **Locations:** `.claude/skills/<name>/SKILL.md` (project), `~/.claude/skills/<name>/SKILL.md` (user), `<plugin>/skills/<name>/SKILL.md` (plugin), managed-policy skills dir. Legacy `.claude/commands/*.md` still works with same frontmatter; skill wins on name collision. Plugin skills use `plugin-name:skill-name` namespace.
- **Frontmatter fields:** `name`, `description`, `when_to_use`, `argument-hint`, `arguments` (positional names), `disable-model-invocation`, `user-invocable`, `allowed-tools`, `model`, `effort`, `context: fork` (run in subagent), `agent` (which subagent type for fork), `hooks`, `paths` (glob auto-load), `shell` (`bash`/`powershell`).
- **String substitutions:** `$ARGUMENTS`, `$ARGUMENTS[N]`, `$N`, `$name` (positional), `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}`, `${CLAUDE_SKILL_DIR}`. Inline shell injection via `` !`cmd` `` or ` ```! ` blocks.
- **Bundled skills:** `/code-review`, `/batch`, `/debug`, `/loop`, `/claude-api`, `/run`, `/verify`, `/run-skill-generator` (v2.1.145+).
- **Visibility settings:** `skillOverrides`, `maxSkillDescriptionChars` (default 1536), `skillListingBudgetFraction` (default 0.01 of context).

## 6. Settings

- **What it does:** Single hierarchical JSON configuration system. `/config` opens a tabbed TUI editor.
- **Scopes (highest → lowest):** Managed > CLI args > Local (`.claude/settings.local.json`) > Project (`.claude/settings.json`) > User (`~/.claude/settings.json`). Permission rules *merge* rather than override.
- **Managed delivery:** server-managed (Claude.ai admin), macOS plist `com.anthropic.claudecode`, Windows registry `HKLM\SOFTWARE\Policies\ClaudeCode`, file-based `/Library/Application Support/ClaudeCode/managed-settings.json` (macOS), `/etc/claude-code/` (Linux/WSL), `C:\Program Files\ClaudeCode\` (Win); drop-in `managed-settings.d/` directory.
- **Live reload:** most keys reload without restart (covers `permissions`, `hooks`, `apiKeyHelper`); `ConfigChange` hook fires. `model` and `outputStyle` apply on restart.
- **Major top-level keys:**
  - *Agent/model:* `agent`, `model`, `modelOverrides`, `availableModels`, `effortLevel`, `alwaysThinkingEnabled`, `showThinkingSummaries`.
  - *Permissions:* `permissions.allow`/`ask`/`deny`/`additionalDirectories`, `permissions.defaultMode`, `disableBypassPermissionsMode`, `skipDangerousModePermissionPrompt`, `disableAutoMode`, `autoMode` (environment/allow/soft_deny/hard_deny rule arrays).
  - *Sandbox:* `sandbox.enabled`, `failIfUnavailable`, `autoAllowBashIfSandboxed`, `excludedCommands`, `allowUnsandboxedCommands`, `filesystem.allow/denyWrite|Read`, `network.allowedDomains`/`deniedDomains`/`httpProxyPort`/`socksProxyPort`/`allowUnixSockets`/`allowLocalBinding`/`allowMachLookup`, `enableWeakerNestedSandbox`/`enableWeakerNetworkIsolation`, `bwrapPath`, `socatPath`.
  - *Hooks:* `hooks`, `disableAllHooks`, `allowManagedHooksOnly`, `allowedHttpHookUrls`, `httpHookAllowedEnvVars`.
  - *MCP:* `enableAllProjectMcpServers`, `enabledMcpjsonServers`, `disabledMcpjsonServers`, `allowedMcpServers`, `deniedMcpServers`, `allowManagedMcpServersOnly`.
  - *Memory:* `autoMemoryEnabled`, `autoMemoryDirectory`, `claudeMd` (managed only), `claudeMdExcludes`.
  - *Plugins:* `enabledPlugins`, `strictKnownMarketplaces`, `blockedMarketplaces`, `strictPluginOnlyCustomization`, `pluginTrustMessage`.
  - *Worktrees:* `worktree.baseRef` (`fresh`/`head`), `worktree.symlinkDirectories`, `worktree.sparsePaths`, `worktree.bgIsolation`.
  - *UI / UX:* `editorMode` (`normal`/`vim`), `viewMode`, `tui` (`default`/`fullscreen`), `autoScrollEnabled`, `spinnerTipsEnabled`/`spinnerTipsOverride`/`spinnerVerbs`, `prefersReducedMotion`, `terminalProgressBarEnabled`, `syntaxHighlightingDisabled`, `awaySummaryEnabled`, `showTurnDuration`, `showClearContextOnPlanAccept`, `language`, `preferredNotifChannel`.
  - *Auth/security:* `apiKeyHelper`, `awsAuthRefresh`/`awsCredentialExport`, `gcpAuthRefresh`, `forceLoginMethod`, `forceLoginOrgUUID`, `forceRemoteSettingsRefresh`, `otelHeadersHelper`, `policyHelper`, `parentSettingsBehavior`.
  - *Telemetry/feedback:* `feedbackSurveyRate`, `skipWebFetchPreflight`.
  - *Auto-update:* `autoUpdatesChannel` (`stable`/`latest`), `minimumVersion`.
  - *Status line & attribution:* `statusLine` (command-based), `attribution.commit`/`pr`, `includeCoAuthoredBy` (deprecated), `prUrlTemplate`.
  - *File suggestion:* `fileSuggestion` (custom command), `respectGitignore`.
  - *Plans:* `plansDirectory`.
  - *Misc:* `env` (env vars passed to subprocesses), `companyAnnouncements`, `cleanupPeriodDays`, `defaultShell` (`bash`/`powershell`), `voice` object, `sshConfigs`, `teammateMode`, `useAutoModeDuringPlan`, `fastModePerSessionOptIn`, `channelsEnabled`, `disableAgentView`, `disableDeepLinkRegistration`, `disableRemoteControl`, `disableSkillShellExecution`, `wslInheritsWindowsSettings`, `includeGitInstructions`.
- **Global config (`~/.claude.json`, *not* settings.json):** `autoConnectIde`, `autoInstallIdeExtension`, `externalEditorContext`, `teammateDefaultModel`.
- **Permission rule grammar:** `Tool` or `Tool(specifier)`; deny → ask → allow; tool-specific patterns for `Bash(npm run *)`, `Read(./.env)`, `WebFetch(domain:example.com)`, MCP and Agent rules.
- **Schema:** `https://json.schemastore.org/claude-code-settings.json`.

## 7. Memory (CLAUDE.md + auto memory)

- **What it does:** Two persistent-context systems. CLAUDE.md = author-written instructions; auto memory = Claude-written notes per repo.
- **CLAUDE.md locations (load order, broad→narrow):** managed policy CLAUDE.md (`/Library/Application Support/ClaudeCode/CLAUDE.md`, `/etc/claude-code/CLAUDE.md`, `C:\Program Files\ClaudeCode\CLAUDE.md`), `~/.claude/CLAUDE.md`, project `./CLAUDE.md` or `./.claude/CLAUDE.md`, local `./CLAUDE.local.md`. Files in ancestor dirs concatenated; nested children load on demand.
- **Imports:** `@path/to/file` syntax, max recursion depth 5; first-time external import triggers approval dialog.
- **AGENTS.md:** not read directly; recommended to `@AGENTS.md` from CLAUDE.md or symlink. `/init` reads `AGENTS.md`, `.cursorrules`, `.windsurfrules`.
- **`.claude/rules/<topic>.md`:** project rules, optional `paths:` frontmatter for glob-scoped activation; symlinks supported; `~/.claude/rules/` for user-level.
- **Auto memory:** `~/.claude/projects/<project>/memory/MEMORY.md` (first 200 lines / 25 KB loaded each session) + topic files loaded on demand. Toggle via `/memory` or `autoMemoryEnabled`/`CLAUDE_CODE_DISABLE_AUTO_MEMORY`. Custom dir via `autoMemoryDirectory` (only honored from user/managed/`--settings`).
- **Block-level HTML comments stripped before injection.**
- **`claudeMdExcludes`** for monorepo skipping (managed CLAUDE.md cannot be excluded).
- **`CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`** to load CLAUDE.md from `--add-dir` paths.
- **Slash commands:** `/memory` (list/edit/toggle), `/init` (generate starter).

## 8. Hooks (reference + guide)

- **What it does:** User-defined shell commands, HTTP endpoints, MCP tools, prompts, or agents that run at lifecycle events; can block, modify, or observe tool use.
- **Event names (lifecycle order roughly):**
  - *Session:* `SessionStart`, `Setup` (with `--init-only`/`--init`/`--maintenance`), `SessionEnd`.
  - *Per turn:* `UserPromptSubmit`, `UserPromptExpansion`, `Stop`, `StopFailure`, `Notification`.
  - *Agentic loop:* `PreToolUse`, `PermissionRequest`, `PermissionDenied`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`.
  - *Subagents/tasks:* `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted`, `TeammateIdle`.
  - *Async/standalone:* `InstructionsLoaded`, `ConfigChange`, `CwdChanged`, `FileChanged`, `WorktreeCreate`, `WorktreeRemove`, `PreCompact`, `PostCompact`, `Elicitation`, `ElicitationResult`.
- **Config structure:** three-level JSON — event → matcher group (filter, e.g. `"Bash"` or `"*"`) → handler array. Handler can be `type: "command"` (shell, stdin JSON, stdout JSON decision or exit codes), `type: "http"` (URL + headers, body is event JSON), `type: "prompt"` (LLM prompt + JSON response schema), `type: "agent"` (subagent), `type: "mcp"` (MCP tool). Optional `if:` (permission-rule-style filter), `args:`, `timeout`, `async: true` (background hooks).
- **Locations:** `~/.claude/settings.json`, `.claude/settings.json`, `.claude/settings.local.json`, managed settings, plugin `hooks/hooks.json`, skill or subagent frontmatter `hooks:`.
- **Decision protocol:** stdout JSON `{"hookSpecificOutput": {"hookEventName": "...", "permissionDecision": "allow|deny|ask", "permissionDecisionReason": "...", "updatedInput": {...}}}`; or exit codes; or HTTP response.
- **Common env vars:** `CLAUDE_PROJECT_DIR`, plus event-specific JSON on stdin.
- **Slash command:** `/hooks` to view configured hooks; `disableAllHooks: true` to kill switch.

## 9. Sub-agents

- **What it does:** Spawn isolated Claude instances with their own system prompt, model, tools, permissions, and (optionally) worktree. Foreground or background.
- **Built-in subagents:** `Explore` (Haiku, read-only, fast lookup), `Plan` (inherits model, read-only, used during plan mode), `general-purpose` (all tools, complex tasks), helper agents `statusline-setup` (Sonnet, for `/statusline`) and `claude-code-guide` (Haiku, for Claude Code Q&A).
- **Locations:** `~/.claude/agents/<name>.md`, `.claude/agents/<name>.md`; `/agents` interactive editor.
- **Frontmatter:** `name`, `description`, `tools`, `disallowedTools`, `model` (`sonnet|opus|haiku|<id>|inherit`), `permissionMode`, `maxTurns`, `skills` (preloaded), `mcpServers`, `hooks`, `memory` (`user|project|local` enables persistent `~/.claude/agent-memory/`), `background: true`, `effort`, `isolation: worktree`, `color`, `initialPrompt`.
- **Invocation:** automatic delegation by description, explicit via `/agents`, `--agent <name>`, `--agents '<json>'`, conversation can spawn (`Task` tool).
- **Forking:** `claude --continue --fork-session` branches a session.
- **Background:** `--bg`, `Ctrl+B`, `claude agents` view, `claude attach`, `claude logs`, `claude stop`, `claude respawn`, `claude rm`. Supervisor daemon (`claude daemon status`).
- **Worktree isolation:** `isolation: worktree` creates `.claude/worktrees/<name>` with configurable `baseRef`, `symlinkDirectories`, `sparsePaths`.

## 10. MCP

- **What it does:** Bring external tools/data into the session via the Model Context Protocol. Three transports.
- **CLI:** `claude mcp add`, `add-json`, `add-from-claude-desktop`, `list`, `get`, `remove`, `serve` (Claude Code itself as an MCP server). Flags `--transport http|sse|stdio`, `--scope local|project|user`, `--env`, `--header`.
- **Slash command:** `/mcp` (status, auth, enable/disable).
- **Config files:** `.mcp.json` (project, committed), `~/.claude.json` (user/local), `--mcp-config <file-or-json>` flag, `--strict-mcp-config` to ignore non-flag sources.
- **Server env:** `CLAUDE_PROJECT_DIR` is injected; `${CLAUDE_PROJECT_DIR}` and `${VAR:-default}` expansion in `.mcp.json`.
- **Reserved name:** `workspace`.
- **Approvals:** per-server prompt; `enableAllProjectMcpServers`, `enabledMcpjsonServers`, `disabledMcpjsonServers`, `allowedMcpServers`, `deniedMcpServers`.
- **Reconnect:** HTTP/SSE 5x backoff (1 s doubling), startup 3 transient retries; stdio not reconnected.
- **Limits / timeouts:** `MCP_TIMEOUT` (server startup), per-server `timeout` (ms, min 1 s, hard 60 s first-byte for HTTP/SSE), `MCP_TOOL_TIMEOUT`, `MAX_MCP_OUTPUT_TOKENS` (default warn ≥10k), `tool.outputLimit` per-tool override.
- **OAuth:** `--mcp-oauth-port`, pre-configured OAuth creds via settings, `OAuth` metadata discovery overrides, scope restriction.
- **Elicitation:** server can request user input mid-tool-call (handled via `Elicitation`/`ElicitationResult` hooks).
- **Resources:** `@server:resource` references.
- **Tool Search:** built-in `ToolSearch` tool defers per-tool schemas until needed. `ENABLE_TOOL_SEARCH=false` to disable; fallback `WaitForMcpServers` tool.
- **Channels:** `claude/channel` capability + `--channels plugin:<name>@<marketplace>` to receive push messages from external systems.
- **Plugin-bundled MCP:** `.mcp.json` at plugin root or inline in `plugin.json`, with `${CLAUDE_PLUGIN_ROOT}` expansion.

## 11. Checkpointing

- **What it does:** Auto-snapshot file state per user prompt; allow restore/summarize from rewind menu.
- **Key surfaces:** `/rewind`, Esc-Esc (when input empty), per-prompt checkpoint list, options: restore code+conversation / restore conversation / restore code / summarize from here / summarize up to here.
- **Lifecycle:** persists across sessions (resumable), pruned with sessions after `cleanupPeriodDays` (default 30).
- **Limitations:** only file-tool edits tracked (not Bash `rm`/`mv`/`cp`); not version control.

## 12. Output styles

- **Built-in styles:** `Default`, `Proactive`, `Explanatory`, `Learning` (last inserts `TODO(human)` markers).
- **Custom styles:** markdown with frontmatter `name`, `description`, `keep-coding-instructions`, `force-for-plugin` at `~/.claude/output-styles/`, `.claude/output-styles/`, managed dir, or plugin `output-styles/`.
- **Activation:** `/config → Output style`, or `outputStyle` setting; takes effect after `/clear` or restart (system prompt is cached).

## 13. IAM / Authentication

- **Slash commands:** `/login`, `/logout`, `/status`.
- **CLI:** `claude auth login` (`--email`, `--sso`, `--console`), `auth logout`, `auth status` (JSON, `--text`), `claude setup-token` (one-year OAuth token printed once).
- **Account types:** Claude Pro/Max/Team/Enterprise (OAuth), Claude Console (API billing), Bedrock/Vertex/Foundry via env.
- **Credential storage:** macOS Keychain, Linux `~/.claude/.credentials.json` mode 0600, Windows `%USERPROFILE%\.claude\.credentials.json`. Path overridable via `CLAUDE_CONFIG_DIR`.
- **Precedence:** cloud provider env > `ANTHROPIC_AUTH_TOKEN` > `ANTHROPIC_API_KEY` > `apiKeyHelper` > `CLAUDE_CODE_OAUTH_TOKEN` > subscription OAuth.
- **Helper script:** `apiKeyHelper` setting, refresh interval `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` (default 5 min or on 401); 10 s slow-helper warning.
- **Org enforcement (managed):** `forceLoginMethod`, `forceLoginOrgUUID`.

## 14. SDK / programmatic (`headless.md` proxy for SDK overview)

- **What it does:** Same engine as `claude` exposed through `-p` (print) plus Python/TypeScript packages (Anthropic-managed Claude Agent SDK).
- **CLI/print-mode flags:** `-p`/`--print`, `--bare` (skip auto-discovery; recommended for CI), `--output-format text|json|stream-json`, `--input-format text|stream-json`, `--json-schema`, `--include-partial-messages`, `--include-hook-events`, `--max-turns`, `--max-budget-usd`, `--no-session-persistence`, `--replay-user-messages`, `--continue`, `--resume`, `--fallback-model`, `--permission-prompt-tool`.
- **Stream events:** `system/init` (model, tools, plugins, plugin_errors), `system/api_retry` (attempt, max_retries, retry_delay_ms, error_status, error category), `system/plugin_install`, text deltas, tool_use, tool_result.
- **JSON result:** includes `result`, `session_id`, `structured_output` (with `--json-schema`), `total_cost_usd`, model breakdown.
- **Bare mode:** skips OAuth/keychain reads; auth must come from `ANTHROPIC_API_KEY` or `apiKeyHelper`. Default tool palette = Bash + file read + file edit.
- **Note:** user-invoked skills/built-in commands like `/commit` are not available in `-p` (describe task instead).
- **Pricing wrinkle:** starting 2026-06-15 SDK/`-p` draws a separate Agent SDK monthly credit on subscription plans.

## 15. IDE integrations (VS Code & forks, JetBrains)

- **Install:** `vscode:extension/anthropic.claude-code`, `cursor:extension/anthropic.claude-code`, Open VSX. JetBrains plugin via Marketplace.
- **Keybindings:** Spark icon, `Cmd+Esc`/`Ctrl+Esc` open panel, `Cmd+Shift+P → Claude Code`, `Option+K`/`Alt+K` insert `@file#5-10` reference, `Ctrl+O` expand thinking blocks.
- **Prompt-box features:** `/` menu (commands, MCP, hooks, memory, permissions, plugins, `/remote-control`, `/usage`); permission-mode selector; context-usage indicator; extended-thinking toggle; multi-line Shift+Enter; `claudeCode.initialPermissionMode` setting.
- **Sessions panel:** history, fuzzy search, rename, remove, resume remote sessions from Claude.ai, multiple tabs/windows.
- **IDE MCP server:** extension exposes a local MCP server (e.g. for selection sharing).
- **Walk-through:** "Claude Code: Open Walkthrough" command-palette entry.
- **VS Code setup commands:** `/terminal-setup` for Shift+Enter binding in non-native terminals.

## 16. GitHub Actions

- **Setup:** `/install-github-app` (interactive) or manual (install GH app `apps/claude` with Contents/Issues/PR read+write, set `ANTHROPIC_API_KEY` secret, copy `claude.yml`).
- **Repository:** `anthropics/claude-code-action`.
- **v1 vs beta:** simplified config; auto mode detection.
- **Auth:** API key, OIDC for Bedrock/Vertex.
- **Workflow inputs:** prompt, allowed tools, MCP config, model.
- **Patterns:** PR review, issue triage, scheduled tasks.

## 17. Dev containers

- **Install:** `ghcr.io/anthropics/devcontainer-features/claude-code:1.0` feature in `devcontainer.json`.
- **Persistence:** volume mount at `/home/<user>/.claude` (`source=claude-code-config-${devcontainerId}`); or `CLAUDE_CONFIG_DIR` env.
- **Policy:** `/etc/claude-code/managed-settings.json` baked via Dockerfile `COPY`; `containerEnv` for env vars; reference firewall script (`init-firewall.sh`) restricts egress; `runArgs` for `NET_ADMIN`/`NET_RAW`.
- **Unattended:** `--dangerously-skip-permissions` (rejected when launched as root).
- **Reference:** `.devcontainer/` in `anthropics/claude-code` repo.

## 18. Headless / non-interactive

- **Bare mode:** `--bare` skips hooks/skills/plugins/MCP/auto-memory/CLAUDE.md auto-discovery; faster, deterministic; becoming the default in a future release.
- **Stdin cap:** 10 MB (as of v2.1.128).
- **Permission flow:** `--allowedTools`, `--permission-mode dontAsk|acceptEdits`, `--permission-prompt-tool` (MCP tool for prompts).

## 19. Troubleshooting

- **Key surfaces:** `/doctor` (installation/settings/MCP/context check), `claude doctor` from shell when CLI won't start, `/heapdump` (writes `~/Desktop/*.heapsnapshot`), `/compact`, `/feedback`, ripgrep override via `USE_BUILTIN_RIPGREP=0`.
- **Symptoms covered:** high CPU/mem, autocompact thrashing, hangs (Ctrl+C, `claude --resume`), WSL search degradation, ripgrep install per OS.

## 20. Costs

- **Slash commands:** `/usage` (session token + cost estimate; subscription bars), `/compact`, `/clear`, `/rewind`, `/context`, `/effort`, `/mcp`, `/model`.
- **Settings:** `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` enables agent teams; `MAX_THINKING_TOKENS` env; `cleanupPeriodDays` for transcripts.
- **Workspace limits:** Console workspace spending caps, organization rate-limit recommendations table (TPM/RPM by team size).
- **Reduction patterns:** hook-side data filtering (PreToolUse `updatedInput`), move long instructions to skills, code-intelligence plugins for typed langs, delegate to subagents, use plan mode.
- **Bedrock/Vertex/Foundry:** no metrics emitted by Claude Code; LiteLLM mentioned as a community option.

## 21. Monitoring usage (OpenTelemetry)

- **Env vars:** `CLAUDE_CODE_ENABLE_TELEMETRY=1`; `OTEL_METRICS_EXPORTER` (`otlp|prometheus|console|none`); `OTEL_LOGS_EXPORTER` (`otlp|console|none`); `OTEL_EXPORTER_OTLP_PROTOCOL` (`grpc|http/json|http/protobuf`), `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, signal-specific overrides; `OTEL_METRIC_EXPORT_INTERVAL` (default 60 s), `OTEL_LOGS_EXPORT_INTERVAL` (default 5 s); content controls `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_TOOL_DETAILS`, `OTEL_LOG_TOOL_CONTENT`, `OTEL_LOG_RAW_API_BODIES` (inline or `file:<dir>`); cardinality `OTEL_METRICS_INCLUDE_SESSION_ID`/`_VERSION`/`_ACCOUNT_UUID`. `otelHeadersHelper` setting for dynamic auth headers; mTLS supported.
- **Standard attributes:** `session.id`, `app.version`, `organization.id`, `user.account_uuid`, `user.account_id`, `user.id` (anonymous), `user.email`, `terminal.type`, plus event-only `prompt.id`, `workspace.host_paths`.
- **Metrics:** `claude_code.session.count`, `lines_of_code.count` (type added/removed), `pull_request.count`, `commit.count`, `cost.usage` (USD; with model/query_source main|subagent|auxiliary, speed=fast, effort, agent/skill/plugin/marketplace.name with redaction for third-party), `token.usage` (type input|output|cacheRead|cacheCreation), `code_edit_tool.decision` (Edit|Write|NotebookEdit; accept|reject; source config|hook|user_*; language), `active_time.total` (user|cli).
- **Events:** `user_prompt`, `tool_result`, `api_request`, `api_error`, `api_request_body`, `api_response_body`, `tool_decision`, `permission_mode_changed`, `auth`, `mcp_server_connection`, `internal_error`, `plugin_installed`, `plugin_loaded`, `skill_activated`, `at_mention`, `api_retries_exhausted`, `hook_registered`, `hook_execution_start`, `hook_execution_complete`, `hook_plugin_metrics`, `compaction`, `feedback_survey`.

## 22. Data usage

- **Policies:** consumer (Free/Pro/Max) opt-in toggle for training; commercial (Team/Enterprise/API) no training without explicit opt-in (Development Partner Program).
- **Retention:** 30 days default; 5 years if consumer opts in; ZDR available for Enterprise.
- **Local cache:** transcripts under `~/.claude/projects/` for `cleanupPeriodDays` days.
- **Feedback:** `/feedback` (5-year retention; uploads conversation history; archive to `~/.claude/feedback-bundles/` on Bedrock/Vertex/Foundry); session quality surveys (rating + optional transcript share, 6-month retention); `feedbackSurveyRate` controls frequency.
- **Telemetry opt-outs:** `DISABLE_TELEMETRY`, `DISABLE_ERROR_REPORTING`, `DISABLE_FEEDBACK_COMMAND`, `CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `CLAUDE_CODE_ENABLE_FEEDBACK_SURVEY_FOR_OTEL`, `DO_NOT_TRACK`.
- **Encryption at rest:** AES-256 per provider table; `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST` flag.
- **WebFetch domain safety:** hostname check against `api.anthropic.com` blocklist (5-minute cache); disable with `skipWebFetchPreflight: true`.

## 23. Background bash

- **What it does:** Run long-running Bash commands without blocking turn.
- **Surfaces:** Ctrl+B to background a running Bash, automatic stderr note when output >5 GB, output retrievable via `Read`, IDs unique per task, auto-cleanup at exit.
- **Config:** `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`.

---

## Canonical built-in tool surface

Anthropic does not publish a single "Tools reference" page; tool names appear across CLI flags, settings examples, sub-agent frontmatter, and hooks docs:

- **File I/O:** `Read`, `Write`, `Edit`, `MultiEdit` (referenced but rarely named), `NotebookEdit`, `Glob`, `Grep`.
- **Shell:** `Bash`, plus a `PowerShell` tool gated by `CLAUDE_CODE_USE_POWERSHELL_TOOL=1` and `defaultShell: "powershell"`.
- **Web:** `WebFetch` (with `WebFetch(domain:...)` permission rules and the domain-safety preflight), `WebSearch`.
- **Orchestration:** `Task`/`TaskCreate` (subagent delegation), `Skill` (invocation), `EnterWorktree`/`ExitWorktree`, `WaitForMcpServers`, `ToolSearch` (MCP tool-deferral mechanism).
- **MCP:** any tool exposed by a connected server, prefixed by server name; permissioned via `mcp__<server>__<tool>` style.

## Slash-command surface

Authoritative list at `/en/commands` (not in the requested URLs), but the corpus mentions explicitly: `/agents`, `/btw`, `/clear`, `/code-review`, `/compact`, `/config`, `/context`, `/debug`, `/desktop`, `/doctor`, `/effort`, `/fast`, `/feedback`, `/focus`, `/heapdump`, `/help`, `/hooks`, `/init`, `/install-github-app`, `/login`, `/logout`, `/loop`, `/mcp`, `/memory`, `/model`, `/plugin` (alias `/plugins`), `/recap`, `/rename`, `/resume`, `/review`, `/rewind`, `/run`, `/run-skill-generator`, `/schedule`, `/security-review`, `/skills`, `/statusline`, `/status`, `/terminal-setup`, `/theme`, `/tui`, `/ultrareview`, `/usage`, `/verify`, `/voice`.

## Surfaces seen but not slotted into a single area

- **Plugins:** complete extension package (skills, hooks, agents, output-styles, MCP servers, statusline). Settings expose `enabledPlugins`, marketplace allowlist/blocklist, `strictPluginOnlyCustomization`, `pluginTrustMessage`. Surfaces: `/plugin`, `claude plugin`, `--plugin-dir`, `--plugin-url`, marketplaces, force-enable-in-managed-settings.
- **Status line:** `statusLine` setting (custom script), `/statusline` slash, `statusline-setup` subagent.
- **Auto mode (permission mode):** classifier-driven approve/deny rules (`autoMode.environment/allow/soft_deny/hard_deny`, `$defaults` reference). `claude auto-mode defaults`/`config`. Separate from `acceptEdits` and `dontAsk`. `disableAutoMode` setting.
- **Permission modes (full list):** `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`. Cycled with Shift+Tab.
- **Worktrees:** `--worktree`, `EnterWorktree` tool, sub-agent `isolation: worktree`, `.worktreeinclude` file, `worktree.*` settings.
- **Channels (research preview):** MCP-server-driven push notifications, `--channels`, `--dangerously-load-development-channels`, `channelsEnabled` managed setting, `allowedChannelPlugins`.
- **Remote Control:** `claude remote-control`, `--remote-control`/`--rc`, `--remote-control-session-name-prefix`, `disableRemoteControl` setting; lets claude.ai/Claude app drive a local session.
- **Routines:** scheduled remote agents on Anthropic infrastructure; `/schedule`, also Desktop "scheduled tasks" run locally; `/loop` for short-interval polling within a session.
- **Teleport:** `claude --teleport` moves a web session into the terminal.
- **Deep links:** `claude-cli://` protocol handler; `disableDeepLinkRegistration` to opt out.
- **Fast mode:** separate from auto/acceptEdits, toggled with Option+O, `/fast`, persisted with `fastModePerSessionOptIn`.
- **Sandboxing model:** macOS Seatbelt + Linux bubblewrap (`bwrap`) + WSL2 + macOS Mach lookup allowlist; per-OS knobs in `sandbox.*`.
- **Onboarding/auto-update:** native installer auto-updates by default; `autoUpdatesChannel`, `minimumVersion`, `DISABLE_AUTOUPDATER`.
- **Heap dump / debug:** `/heapdump`, `--debug`, `--debug-file`, `CLAUDE_CODE_DEBUG_LOGS_DIR`.

---

## Source pages (fetched 2026-05-24)

All pages live at `https://docs.claude.com/en/docs/claude-code/<slug>`.
Markdown export at `https://code.claude.com/docs/en/<slug>.md`.

| Page | Status | Notes |
|---|---|---|
| overview | ✓ | |
| quickstart | ✓ | |
| cli-reference | ✓ | exhaustive flag table |
| interactive-mode | ✓ | full key map |
| slash-commands | ✓ | merged into skills |
| skills | ✓ | |
| settings | ✓ | ~80 top-level keys |
| memory | ✓ | |
| hooks | ✓ | 25+ event types |
| hooks-guide | ✓ | tutorial companion |
| sub-agents | ✓ | |
| mcp | ✓ | |
| checkpointing | ✓ | |
| output-styles | ✓ | |
| iam | ✓ | |
| sdk/overview | ✗ | HTML-only; skipped (use headless.md) |
| ide-integrations | ✓ | VS Code focus |
| github-actions | ✓ | |
| devcontainer | ✓ | |
| headless | ✓ | proxies SDK |
| troubleshooting | ✓ | |
| costs | ✓ | |
| monitoring-usage | ✓ | metrics + events + traces |
| data-usage | ✓ | |
