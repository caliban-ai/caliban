# Claude Code documented-capability inventory

> **Static snapshot — captured 2026-08-15.**
>
> Structured snapshot of Claude Code's documented surface, captured from
> the public docs at `docs.claude.com/en/docs/claude-code/*`. This is the
> source feeding [`parity-gap-matrix.md`](parity-gap-matrix.md). It is
> intentionally a point-in-time capture, not a live mirror.
>
> **Currency marker:** newest version referenced anywhere in the corpus at
> capture was **`v2.1.233`** (2026-08-14, per `changelog`). Version gates cited
> below (`v2.1.NNN+`) are upstream's own annotations — they are the best
> available signal for dating a feature. Use this marker to gauge drift on the
> next re-baseline. The prior capture (2026-05-24) predates *every* `v2.1.18x+`
> gate in this document.
>
> **Fetch note:** the canonical machine-readable index is now
> `https://code.claude.com/docs/llms.txt`. Per-page markdown exports live at
> `https://code.claude.com/docs/en/<slug>.md` (preferred — clean, complete);
> the HTML pages at `https://docs.claude.com/en/docs/claude-code/<slug>` render
> the same content. **Three slugs from the 2026-05-24 capture have moved**
> (see §0). The doc site has grown from ~24 pages to **100+**; §25 lists the
> pages that exist now but did not then.
>
> **Re-baseline cadence:** refresh manually before each parity-prioritization
> review. When refreshing, re-fetch the upstream docs, update the sections
> below, bump the snapshot date + currency marker in this header, and propagate
> any new rows into `parity-gap-matrix.md` in the same commit.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines
> name the canonical configuration mechanism. **[NEW]** marks surface added
> since the 2026-05-24 capture; **[GONE]** marks surface that capture recorded
> and upstream has since removed or renamed.

## 0. What moved since 2026-05-24

Structural changes to the doc site itself, recorded so the next refresh does
not chase dead slugs:

| 2026-05-24 slug | 2026-08-15 |
|---|---|
| `iam` | **`authentication`** |
| `ide-integrations` | **split → `vs-code` + `jetbrains`** (old slug serves the VS Code page) |
| `sdk/overview` | **`agent-sdk/overview`** (whole SDK tree is `agent-sdk/*`, ~30 pages) |
| `slash-commands` | byte-identical to **`skills`** (folded in); built-in commands now documented on the new **`commands`** page |
| `troubleshooting` | now a router; content split to `troubleshoot-install`, `debug-your-config`, `errors`, `vs-code`, `jetbrains` |
| `sub-agents` (background CLI half) | moved to the new **`agent-view`** page |
| `sub-agents` (worktree keys) | moved to the new **`worktrees`** page |
| `mcp` (managed half) | moved to the new **`managed-mcp`** page |
| `github-actions` (cloud providers) | moved to **`github-actions-cloud-providers`** |

Skills now formally follow the **Agent Skills open standard**
(`agentskills.io`) rather than being a Claude-Code-only format.

## 1. Overview / "Use Claude Code everywhere"

- **What it does:** Positions Claude Code as a single agentic engine accessible from multiple surfaces. Each surface shares CLAUDE.md, settings, MCP servers.
- **Key surfaces:** Terminal CLI (`claude`), VS Code / Cursor / forks extension, JetBrains plugin, Desktop app (macOS/Win/Linux), Web (`claude.ai/code`), **the Claude app for iOS *and Android*** [NEW: Android], Slack, GitHub Actions, GitLab CI/CD, Chrome, Remote Control, Routines, Dispatch, Channels, Background agents.
- **[NEW] surfaces / concepts named on this page:** **Cowork**, **Agent SDK**, **dynamic workflows**, **`claude --cloud`**, **`claude --teleport`**, **`/desktop`** handoff, **`/schedule`** (Routines from the CLI), **Desktop scheduled tasks**, **`/loop`**, **GitHub Code Review**, **MCP quickstart**, **auto memory**, **background agents / Agent View**, a **glossary** with a formal "surface" concept.
- **Install methods:** native installer `curl -fsSL https://claude.ai/install.sh | bash`; **[NEW]** Windows PowerShell `irm https://claude.ai/install.ps1 | iex` and Windows CMD `install.cmd`; Homebrew now **two casks** — `claude-code` (stable) and `claude-code@latest` (latest), via `brew install --cask`; WinGet (`Anthropic.ClaudeCode`), apt/dnf/apk.
- **Desktop downloads:** macOS (universal), Windows x64, **[NEW] Windows ARM64**. **[GONE]** Linux desktop download is no longer listed.
- **[GONE]** `--bg` / `claude agents` are no longer mentioned on Overview (they moved to `cli-reference` / `agent-view`).
- **Config:** install scripts; per-surface install; auth via `claude` first run.

## 2. Quickstart

- **What it does:** Tutorial walking from install → `/login` → first session → first edit → git workflow → bug fix → tests/refactor.
- **Key surfaces:** `claude`, `claude "task"`, `claude -p`, `claude -c`, `claude -r`, `/clear`, `/help`, **[NEW] `/exit`**, Tab completion, ↑ history, Shift+Tab to cycle permission modes.
- Essential commands are now split into explicit **Shell commands** vs **Session commands** tables. **[GONE]** `/login` and `/resume` dropped from those tables (still in prose).
- **Config:** relies on CLI commands and `/config` later.

## 3. CLI reference

- **What it does:** Authoritative list of `claude` subcommands and flags. Notes that `--help` is incomplete.
- **Subcommands (2026-05-24 set, all retained):** `claude`, `claude "query"`, `claude -p "query"`, `claude -c`, `claude -r "<session>"`, `claude update`, `claude install [version]`, `claude auth login|logout|status`, `claude agents`, `claude attach <id>`, `claude auto-mode defaults|config`, `claude daemon status`, `claude logs <id>`, `claude mcp ...`, `claude plugin ...` (alias `plugins`), `claude project purge`, `claude remote-control`, `claude respawn <id>`, `claude rm <id>`, `claude setup-token`, `claude stop|kill <id>`, `claude ultrareview` (v2.1.227+).
- **[NEW] subcommands:**
  - `claude gateway --config gateway.yaml` — self-hosted **Claude apps gateway** (SSO/policy in front of Bedrock, Google Cloud Agent Platform, Microsoft Foundry). v2.1.195+
  - `claude doctor` — read-only install/settings diagnostics from the shell, distinct from in-session `/doctor`.
  - `claude import [codex|gemini]` — import config from another coding agent; `--dry-run`, `--yes`. v2.1.213+
  - `claude mcp login <name>` / `claude mcp logout <name>` — MCP OAuth from the CLI; `--no-browser`. v2.1.186+
  - `claude mcp reset-project-choices`
  - `claude auto-mode reset` (`-y`/`--yes`) — removes `autoMode` from user settings. v2.1.212+
  - `claude daemon stop --any [--keep-workers]`
  - `claude self-hosted-runner {setup,doctor,orchestrator}` — v2.1.224+
  - `claude auto-mode defaults --label <prefix>` — v2.1.208+
- **Flags (2026-05-24 set):** `--add-dir`, `--agent`, `--agents <json>`, `--allow-dangerously-skip-permissions`, `--allowedTools`, `--append-system-prompt[-file]`, `--bare`, `--betas`, `--bg`, `--channels`, `--chrome`, `--continue`/`-c`, `--dangerously-skip-permissions`, `--debug[-file]`, `--disable-slash-commands`, `--disallowedTools`, `--effort`, `--exclude-dynamic-system-prompt-sections`, `--fallback-model`, `--fork-session`, `--from-pr`, `--ide`, `--init` / `--init-only` / `--maintenance` (now three separate rows), `--include-hook-events`, `--include-partial-messages`, `--input-format`, `--json-schema`, `--max-budget-usd` (enforcement v2.1.217+), `--max-turns`, `--mcp-config`, `--model`, `--name`/`-n`, `--no-session-persistence`, `--output-format`, `--permission-mode`, `--permission-prompt-tool`, `--plugin-dir`, `--plugin-url`, `--print`/`-p`, `--remote`, `--remote-control`/`--rc`, `--replay-user-messages`, `--resume`/`-r`, `--session-id`, `--setting-sources`, `--settings`, `--strict-mcp-config`, `--system-prompt[-file]`, `--teleport`, `--teammate-mode`, `--tmux`, `--tools`, `--verbose`, `--version`/`-v`, `--worktree`/`-w`.
- **[NEW] flags:**
  - `--advisor <model>` (`fable`/`opus`/`sonnet`/full ID) — server-side **advisor tool**; overrides `advisorModel`.
  - `--append-subagent-system-prompt` — appends to every subagent's system prompt; `-p` only. v2.1.205+
  - `--autocompact <auto|tokens>` (e.g. `500k`). v2.1.221+
  - `--ax-screen-reader` — screen-reader output; forces the classic renderer. v2.1.181+
  - `--cloud` — task description, `session_…`/`cse_…` ID, or a claude.ai/code URL.
  - `--dangerously-load-development-channels`
  - `--environment <ccpool_…>` + `--ref <branch>` — self-hosted environments. v2.1.224+
  - `--exec` — run a shell command as a PTY-backed background job (pairs with `--bg`).
  - `--forward-subagent-text` — needs `--print` + `--output-format stream-json`. v2.1.211+ (nested forwarding v2.1.219+)
  - `--no-chrome`
  - `--prompt-suggestions` — emits `prompt_suggestion` messages; needs `-p`, `stream-json`, `--verbose`.
  - `--remote-control-session-name-prefix <prefix>`
  - `--safe-mode` — disables all customizations for triage; sets `CLAUDE_CODE_SAFE_MODE`.
  - `--background` (documented long alias for `--bg`); `--allowed-tools` / `--disallowed-tools` kebab-case aliases.
- **[NEW] values on existing flags:**
  - `--effort` gained **`xhigh`** and **`ultracode`** (v2.1.203+). Full set: `low`, `medium`, `high`, `xhigh`, `max`, `ultracode`.
  - `--permission-mode` gained **`manual`** (alias for `default`, the UI label). v2.1.200+
  - `--teammate-mode` gained **`iterm2`**. v2.1.186+ Full set: `in-process` (default), `auto`, `tmux`, `iterm2`.
  - `--model` aliases now `sonnet`, `opus`, `haiku`, **`fable`**.
  - `--tmux` accepts optional `classic`; requires `--worktree`; uses iTerm2 native panes when available.
  - `--worktree`/`-w` accepts `#<number>`, a GitHub PR URL, **or a GitLab MR URL** (GitLab needs v2.1.233+).
  - `--fallback-model` is now a comma-separated **chain**.
  - `--debug` takes a **category filter** (e.g. `mcp,startup`, `!1p`).
- **[GONE] / deprecated:**
  - **`--enable-auto-mode` removed in v2.1.111** (tombstone row retained; use `--permission-mode auto`).
  - **`--remote` deprecated**, superseded by `--cloud`.

## 4. Interactive mode

- **What it does:** TUI keybindings, vim mode, transcript viewer, background bash, prompt suggestions, side questions, task list, session recap, PR badge.
- **Key bindings (2026-05-24 set, all retained):** Ctrl+C, Ctrl+X Ctrl+K, Ctrl+D, Ctrl+G / Ctrl+X Ctrl+E, Ctrl+L, Ctrl+O, Ctrl+R, Ctrl+B, Ctrl+T, Esc, Esc-Esc, Shift+Tab, Option+P, Option+T, Option+O; readline bindings (Ctrl+A/E/K/U/W/Y, Alt+B/F, Alt+Y); multiline (`\`+Enter, Option+Enter, Shift+Enter native in iTerm2/WezTerm/Ghostty/Kitty/Warp/Apple Terminal/Win Term, Ctrl+J fallback); `/`, `!`, `@`; transcript keys `?`, `{`/`}`, Ctrl+E, `[`, `v`, `q`/Esc; voice dictation Space.
- **[NEW] key bindings:**
  - `Ctrl+V` / `Cmd+V` (iTerm2) / `Alt+V` (Windows & WSL) — **paste image from clipboard**, inserts an `[Image #N]` chip.
  - `Ctrl+S` at top level — **stash / restore prompt** (new meaning).
  - `Ctrl+Z` — suspend (Unix; `fg` resumes).
  - Left/Right — cycle dialog tabs. Up/Down or `Ctrl+P`/`Ctrl+N` — cursor, then history at row boundaries.
  - `Alt+M` — Windows fallback for Shift+Tab when VT input mode is unavailable.
  - `Ctrl+_` / `Ctrl+Shift+-` — undo last input edit.
  - `Ctrl+T` inside the `/theme` picker — toggle syntax highlighting.
  - `:` at start — **emoji shortcode** completion (v2.1.217+); `emojiCompletionEnabled: false` disables.
  - `?` on empty input — toggle the shortcut help panel.
  - `Ctrl+E` on a Bash/PowerShell permission prompt — model-generated command explanation (`permissionExplainerEnabled`).
  - `Tab` / Right — accept a prompt suggestion.
- **Changed behavior:** `Esc-Esc` now clears the input draft when text is present (rewind only on empty input). `Ctrl+L` twice within 2 s runs `/clear` in fullscreen. `@` also suggests **live sessions on the machine for cross-session messaging** (v2.1.232+). Shift+Tab cycle documented as `default` (labeled **Manual**) → `acceptEdits` → `plan` → `bypassPermissions` → `auto`.
- **Vim mode:** full normal/insert/visual with motions and text objects; block-visual (Ctrl+V) still **not** supported. **[NEW] `vimInsertModeRemaps`** — map a two-char INSERT-mode sequence to `<Esc>` (e.g. `"jj": "<Esc>"`), 1 s window, only `"<Esc>"` supported, **read only from user / `--settings` / managed settings — project settings ignored**. v2.1.208+
- **[NEW] fullscreen rendering** as a first-class concept: `Ctrl+R` opens a **search dialog** in fullscreen (`Ctrl+S` cycles session/project/all-projects scope; Enter/Tab accept, Esc cancel). Classic inline `Ctrl+R` **always searches all projects**. `/tui` reports the active renderer.
- **[NEW] rebindable transcript keys** via `transcript:toggleShowAll` / `transcript:exit` (see the new `keybindings` page).
- **[NEW] shell mode (`!`) changes:** Claude now **responds to the output automatically** (`respondToBashCommands: false` restores pre-v2.1.186 behavior); Tab history autocomplete; live file-path autocomplete triggered by `/` (v2.1.193+); auto-entry when pasting text starting with `!`.
- **Background bash:** 5 GB cap retained; **[NEW]** memory-pressure reaping on macOS/Linux after 30 min idle (v2.1.193+, `CLAUDE_CODE_DISABLE_BG_SHELL_PRESSURE_REAP=1`); subagent-owned background commands terminated after 60 min (`CLAUDE_SUBAGENT_BG_SHELL_MAX_MS`).
- **[NEW] `/btw` is now a threaded overlay:** replays the newest 20 exchanges; keys Space/Enter/Esc, Up/Down, **Left/Right** (step between answers, v2.1.187+), **`c`** (copy as Markdown), **`f`** (fork into a new session), **`x`** (clear thread). `/btw` with no question reopens the overlay (v2.1.212+).
- **[NEW] task list default flipped:** on Opus 4.8 / Sonnet 5 / Fable 5 / Mythos 5 and later the task list is **empty by default**; opt in with `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`. `CLAUDE_CODE_TASK_LIST_ID` retained.
- **[NEW] prompt suggestions are off by default** in interactive mode when feature flags aren't evaluated; new `promptSuggestionEnabled` setting; `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false` takes precedence.
- **Other:** command history per-cwd; PR badge polled every 60 s, requires `gh`, **[NEW] `FORCE_HYPERLINK=0`** renders it as plain text; `/recap`; **[NEW] `/voice tap`** for tap-to-toggle dictation.
- **[NEW] env vars named here:** `CLAUDE_CODE_DISABLE_BG_SHELL_PRESSURE_REAP`, `CLAUDE_SUBAGENT_BG_SHELL_MAX_MS`, `CLAUDE_CODE_ENABLE_TODO_TOOLS`, `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST`, `DISABLE_GROWTHBOOK`, `FORCE_HYPERLINK`.

## 5. Slash commands / Skills (merged page)

> `slash-commands` and `skills` now serve the **identical** page. Built-in
> commands moved to the new **`commands`** page. Skills follow the **Agent
> Skills open standard** (`agentskills.io`).

- **Locations:** `.claude/skills/<name>/SKILL.md` (project), `~/.claude/skills/<name>/SKILL.md` (user), `<plugin>/skills/<name>/SKILL.md` (plugin), managed-policy skills dir. Legacy `.claude/commands/*.md` still works; skill wins on name collision. Plugin skills use `plugin-name:skill-name`.
- **Frontmatter (2026-05-24 set):** `name`, `description`, `when_to_use`, `argument-hint`, `arguments`, `disable-model-invocation`, `user-invocable`, `allowed-tools`, `model`, `effort`, `context: fork`, `agent`, `hooks`, `paths`, `shell` (`bash`/`powershell`).
- **[NEW] frontmatter:** `disallowed-tools`; **`background`** (only with `context: fork`; `false` waits for the result in-turn; default `true`, v2.1.218+); `metadata` (free-form, ignored); `license` and `compatibility` (Agent Skills spec fields, accepted but not acted on).
- **[NEW] value changes:** booleans accept `yes`/`no`/`on`/`off`/`1`/`0` in any case (pre-v2.1.218 only `true`/`false`); `effort` gained `xhigh`; `model` accepts `inherit`.
- **String substitutions:** `$ARGUMENTS`, `$ARGUMENTS[N]`, `$N`, `$name`, `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}` (values now `low|medium|high|xhigh|max`; ultracode reports as `xhigh`), `${CLAUDE_SKILL_DIR}`; **[NEW] `${CLAUDE_PROJECT_DIR}`**, **[NEW] `${CLAUDE_PLUGIN_ROOT}`**. Inline shell via `` !`cmd` `` or ` ```! ` blocks.
- **Bundled skills:** `/code-review` (alias **`/review`**), `/batch`, `/debug`, `/loop`, `/claude-api`, `/run`, `/verify`, `/run-skill-generator` (v2.1.145+; `/verify` self-recording v2.1.200+). **[NEW] `/doctor` is now a bundled skill** (was a built-in command before v2.1.205); it survives `disableBundledSkills` and is hidden only via `DISABLE_DOCTOR_COMMAND` or `skillOverrides: {"doctor": "off"}`. Built-ins reachable through the Skill tool: `/init`, `/security-review`.
- **Visibility settings:** `skillOverrides` (states `on`, `name-only`, `user-invocable-only`, `off`; **[NEW]** `off` also hides from Remote Control and Agent SDK command lists, v2.1.199+), **`skillListingMaxDescChars`** (default 1536 — **this is the rename of `maxSkillDescriptionChars`**), `skillListingBudgetFraction` (default 0.01), **[NEW] `disableBundledSkills`**, **[NEW] `SLASH_COMMAND_TOOL_CHAR_BUDGET`** env var.
- **[NEW] mechanics:** synced skills from claude.ai land in `~/.claude/skills/synced/` (folder name **reserved**; gated by `CLAUDE_CODE_SYNC_SKILLS` non-interactively). **Nested/directory-qualified skills** — `apps/web/.claude/skills/deploy/SKILL.md` → `/apps/web:deploy`, loaded lazily when Claude touches that subtree. **Live change detection** — SKILL.md text edits apply without restart (`hooks/`, `.mcp.json`, `agents/`, `output-styles/` need `/reload-plugins`). Skill dirs may be symlinks; a skill folder containing `.claude-plugin/plugin.json` loads as a plugin named `<name>@skills-dir`. Plugin skill `name` now replaces only the **last segment**, keeping the plugin prefix (pre-v2.1.216 it replaced the whole name). `disable-model-invocation: true` also blocks scheduled-task prompts (v2.1.196+) and subagent preloading. Cowork/cloud/routine sessions don't read `~/.claude/skills/`. Only six frontmatter fields survive claude.ai upload / the Skills API (`name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools`) — anything else is a hard error. Evaluation tooling ships as the **`skill-creator`** plugin.

## 6. Settings

- **What it does:** Single hierarchical JSON configuration system. `/config` opens a tabbed TUI editor.
- **Scopes (highest → lowest):** Managed > CLI args > Local (`.claude/settings.local.json`) > Project (`.claude/settings.json`) > User (`~/.claude/settings.json`). Permission rules *merge* rather than override.
- **Managed delivery:** **[NEW] server-managed settings** delivered at sign-in from the claude.ai admin console or a self-hosted **Claude apps gateway**; macOS plist `com.anthropic.claudecode`; Windows registry `HKLM\SOFTWARE\Policies\ClaudeCode` **[NEW] plus user-level `HKCU\SOFTWARE\Policies\ClaudeCode`** (lowest policy priority); file-based `/Library/Application Support/ClaudeCode/managed-settings.json` (macOS), `/etc/claude-code/` (Linux/WSL), `C:\Program Files\ClaudeCode\` (Win); drop-in `managed-settings.d/`; **[NEW] `managed-mcp.json`** alongside. **[GONE]** legacy `C:\ProgramData\ClaudeCode\managed-settings.json` unsupported as of v2.1.75.
- **[NEW] managed-tier concepts:** precedence *within* the managed tier; documented exceptions to managed precedence; **array settings merge across scopes** (with `fallbackModel` and `availableModels` explicitly exempt).
- **Live reload:** most keys reload without restart (covers `permissions`, `hooks`, `apiKeyHelper`); `ConfigChange` hook fires. `model` and `outputStyle` apply on restart.
- **[NEW] `.claude/settings.local.json` resolves to the git repo root** (via worktrees) rather than the starting directory (v2.1.211+); Claude Code auto-adds `**/.claude/settings.local.json` to global git excludes. Five most recent config backups retained.
- **Major top-level keys** (2026-05-24 groups retained in full; additions marked):
  - *Agent/model:* `agent`, `model`, `modelOverrides`, `availableModels`, `effortLevel`, `alwaysThinkingEnabled`, `showThinkingSummaries`; **[NEW]** `advisorModel`, `fallbackModel`, `enforceAvailableModels`, `switchModelsOnFlag`, `ultracode` (not read from settings.json — set via `/effort ultracode` or `--settings`), `fastMode`, `outputStyle`, `subagentStatusLine`.
  - *Context/compaction:* **[NEW]** `autoCompactEnabled`, `autoCompactWindow` (100 000–1 000 000 tokens).
  - *Permissions:* `permissions.allow`/`ask`/`deny`/`additionalDirectories`, `permissions.defaultMode`, `disableBypassPermissionsMode`, `skipDangerousModePermissionPrompt`, `disableAutoMode`, `autoMode`; **[NEW]** `allowManagedPermissionRulesOnly` (managed-only), `permissionExplainerEnabled`, `askUserQuestionTimeout` (default `"never"`), `autoMode.classifyAllShell`.
  - *Sandbox:* `sandbox.enabled`, `failIfUnavailable`, `autoAllowBashIfSandboxed`, `excludedCommands`, `allowUnsandboxedCommands`, `filesystem.allow/denyWrite|Read`, `network.allowedDomains`/`deniedDomains`/`httpProxyPort`/`socksProxyPort`/`allowUnixSockets`/`allowLocalBinding`/`allowMachLookup`, `enableWeakerNestedSandbox`/`enableWeakerNetworkIsolation`, `bwrapPath`, `socatPath`; **[NEW]** `sandbox.credentials` (+ `allowPlaintextInject`, `awsPairs`, `envVars`, `files`, `sigv4`), `allowAppleEvents`, `filesystem.allowManagedReadPathsOnly`, `filesystem.disabled`, `network.allowAllUnixSockets`, `network.allowManagedDomainsOnly`, `network.strictAllowlist`, `network.tlsTerminate`, `processWrapper`.
  - *Hooks:* `hooks`, `disableAllHooks`, `allowManagedHooksOnly`, `allowedHttpHookUrls`, `httpHookAllowedEnvVars`.
  - *MCP:* `enableAllProjectMcpServers`, `enabledMcpjsonServers`, `disabledMcpjsonServers`, `allowedMcpServers`, `deniedMcpServers`, `allowManagedMcpServersOnly`; **[NEW]** `allowAllClaudeAiMcps` (managed-only), `disableClaudeAiConnectors`.
  - *Memory:* `autoMemoryEnabled`, `autoMemoryDirectory`, `claudeMd` (managed only), `claudeMdExcludes`.
  - *Skills/workflows:* **[NEW]** `disableBundledSkills`, `skillListingMaxDescChars`, `disableWorkflows`, `workflowKeywordTriggerEnabled`, `workflowSizeGuideline` (default `medium`).
  - *Plugins:* `enabledPlugins`, `strictKnownMarketplaces`, `blockedMarketplaces`, `strictPluginOnlyCustomization`, `pluginTrustMessage`; **[NEW]** `extraKnownMarketplaces`, `defaultEnabled`, `disableCommandPluginSources` (managed-only, v2.1.229+), `disableSideloadFlags` (managed-only — rejects `--plugin-dir`, `--plugin-url`, `--agents`, `--mcp-config`), `pluginSuggestionMarketplaces` (managed-only).
  - *Worktrees:* `worktree.baseRef` (`fresh`/`head`), `worktree.symlinkDirectories`, `worktree.sparsePaths`, `worktree.bgIsolation` (all now explicitly `worktree.`-prefixed).
  - *Remote Control / notifications / cross-session:* **[NEW]** `remoteControlAtStartup`, `agentPushNotifEnabled`, `inputNeededNotifEnabled`, `dialogExpiry` (default `"5m"`), `crossSessionInbound`, `isolatePeerMachines`, `allowedChannelPlugins` (managed-only).
  - *Desktop surfaces (managed-only):* **[NEW]** `browserExternalPageTools`, `disableBrowserExternalNavigation`, `disableMobileSimulatorTools`.
  - *Version gating (managed-only):* **[NEW]** `requiredMinimumVersion`, `requiredMaximumVersion`.
  - *UI / UX:* `editorMode` (`normal`/`vim`), `viewMode`, `tui`, `autoScrollEnabled`, `spinnerTipsEnabled`/`spinnerTipsOverride`/`spinnerVerbs`, `prefersReducedMotion`, `terminalProgressBarEnabled`, `syntaxHighlightingDisabled`, `awaySummaryEnabled`, `showTurnDuration`, `showClearContextOnPlanAccept`, `language`, `preferredNotifChannel`; **[NEW]** `theme` (`auto`, `dark`, `light`, `dark-daltonized`, `light-daltonized`, `dark-ansi`, `light-ansi`, or a custom ref), `verbose`, `axScreenReader`, `emojiCompletionEnabled`, `wheelScrollAccelerationEnabled`, `vimInsertModeRemaps`, `promptSuggestionEnabled`, `respondToBashCommands`, `footerLinksRegexes`, `fileCheckpointingEnabled`, `voiceEnabled`.
  - *Artifacts:* **[NEW]** `disableArtifact`, `enableArtifact`.
  - *Auth/security:* `apiKeyHelper`, `awsAuthRefresh`/`awsCredentialExport`, `gcpAuthRefresh`, `forceLoginMethod`, `forceLoginOrgUUID`, `forceRemoteSettingsRefresh`, `otelHeadersHelper`, `policyHelper`, `parentSettingsBehavior`; **[NEW]** `forceLoginGatewayUrl`, `remote.defaultEnvironmentId`.
  - *Telemetry/feedback:* `feedbackSurveyRate`, `skipWebFetchPreflight`.
  - *Auto-update:* `autoUpdatesChannel` (`stable`/`latest`), `minimumVersion`.
  - *Status line & attribution:* `statusLine`, `attribution.commit`/`pr`/**[NEW] `sessionUrl`**, `includeCoAuthoredBy` (**[NEW] now deprecated** — `attribution` takes precedence), `prUrlTemplate`.
  - *File suggestion:* `fileSuggestion`, `respectGitignore`. *Plans:* `plansDirectory`.
  - *Misc:* `env`, `companyAnnouncements`, `cleanupPeriodDays`, `defaultShell`, `voice`, `sshConfigs`, `teammateMode`, `useAutoModeDuringPlan`, `fastModePerSessionOptIn`, `channelsEnabled`, `disableAgentView`, `disableDeepLinkRegistration`, `disableRemoteControl`, `disableSkillShellExecution`, `wslInheritsWindowsSettings`, `includeGitInstructions`.
- **Global config (`~/.claude.json`, *not* settings.json):** `autoConnectIde`, `autoInstallIdeExtension`, `externalEditorContext`, `teammateDefaultModel`; **[NEW]** `diffTool` (`auto`|`terminal`), `permissionExplainerEnabled`.
- **[NEW] sections:** Footer link badges; Compute managed settings with a policy helper; Verify active settings (via the `/status` "Setting sources" line); Invalid entries in managed settings; When edits take effect.
- **Permission rule grammar:** `Tool` or `Tool(specifier)`; deny → ask → allow; tool-specific patterns for `Bash(npm run *)`, `Read(./.env)`, `WebFetch(domain:example.com)`, MCP and Agent rules.
- **Schema:** `https://json.schemastore.org/claude-code-settings.json` — **[NEW]** now carries an explicit caveat that it lags recent releases.

## 7. Memory (CLAUDE.md + auto memory)

- **What it does:** Two persistent-context systems. CLAUDE.md = author-written instructions; auto memory = Claude-written notes per repo.
- **CLAUDE.md locations (load order, broad→narrow):** managed policy CLAUDE.md (`/Library/Application Support/ClaudeCode/CLAUDE.md`, `/etc/claude-code/CLAUDE.md`, `C:\Program Files\ClaudeCode\CLAUDE.md`), `~/.claude/CLAUDE.md`, project `./CLAUDE.md` or `./.claude/CLAUDE.md`, local `./CLAUDE.local.md`. Ancestors concatenated; nested children load on demand. The `claudeMd` key inlines the same content into `managed-settings.json` and is honored **only** in managed/policy settings.
- **Imports:** `@path/to/file` syntax, **[CHANGED] maximum depth is now four hops** (was 5); first-time external import triggers approval. **[NEW]** import parsing skips Markdown code spans and fenced blocks — `` `@README` `` stays literal.
- **[CHANGED] `/init` sources:** now reads **Copilot rules (`.github/copilot-instructions.md`)** and Cursor rules (`.cursor/rules/`, `.cursorrules`) by default. **`CLAUDE_CODE_NEW_INIT=1`** additionally enables `AGENTS.md`, `.devin/rules/`, `.windsurf/rules/` or `.windsurfrules`, and `.clinerules` — i.e. `AGENTS.md` / `.windsurfrules` are **no longer read unconditionally**. **[NEW] `/import`** (and `claude import`) migrates another agent's config. v2.1.213+
- **`.claude/rules/<topic>.md`:** project rules, optional `paths:` frontmatter for glob-scoped activation; symlinks supported; `~/.claude/rules/` for user level. **[NEW]** rules *without* `paths` load at launch at the same priority as `.claude/CLAUDE.md`; `.md` files are discovered recursively.
- **Auto memory:** `~/.claude/projects/<project>/memory/MEMORY.md` (first 200 lines / 25 KB loaded each session) + topic files on demand. Toggle via `/memory` or `autoMemoryEnabled`/`CLAUDE_CODE_DISABLE_AUTO_MEMORY`. **[NEW]** `autoMemoryDirectory` must be absolute or `~/`-prefixed, is readable from any scope, and is gated by workspace trust when set in project settings; the memory dir is **excluded from the `cleanupPeriodDays` sweep**; a **`modified`** ISO-8601 frontmatter field is written on each memory write (v2.1.214+); subagents don't inherit main-conversation auto memory except forks, and can have their own via the subagent `memory` field; new error surface "memory index is over its read limit".
- **Block-level HTML comments stripped before injection** — **[NEW]** and they don't count toward MEMORY.md limits (v2.1.211+).
- **`claudeMdExcludes`** for monorepo skipping (managed CLAUDE.md cannot be excluded).
- **`CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`** — **[NEW]** now loads `CLAUDE.md`, `.claude/CLAUDE.md`, `.claude/rules/*.md`, **and** `CLAUDE.local.md`.
- **Slash commands:** `/memory` (**[NEW]** no longer blocks on GUI editors, v2.1.216+), `/init`, **[NEW] `/import`**.
- **[NEW] sections:** CLAUDE.md vs auto memory; Manage CLAUDE.md for large teams; Troubleshoot memory issues.

## 8. Hooks (reference + guide)

- **What it does:** User-defined shell commands, HTTP endpoints, MCP tools, prompts, or agents that run at lifecycle events; can block, modify, or observe tool use. **31 documented events.**
- **Event names:**
  - *Session:* `SessionStart`, `Setup` (with `--init-only`/`--init`/`--maintenance`), `SessionEnd`.
  - *Per turn:* `UserPromptSubmit`, `UserPromptExpansion`, `Stop`, `StopFailure`, `Notification`, **[NEW] `MessageDisplay`** (10 s timeout).
  - *Agentic loop:* `PreToolUse`, `PermissionRequest`, `PermissionDenied`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`.
  - *Subagents/tasks:* `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted`, `TeammateIdle`.
  - *Async/standalone:* `InstructionsLoaded`, `ConfigChange`, `CwdChanged`, `FileChanged`, **[NEW] `DirectoryAdded`**, `WorktreeCreate`, `WorktreeRemove`, `PreCompact`, `PostCompact`, `Elicitation`, `ElicitationResult`.
- **Config structure:** three-level JSON — event → matcher group → handler array. Handler types `command`, `http`, `prompt`, `agent`, and **[RENAMED] `mcp_tool`** (was `mcp`; fields `server` — accepts scoped `plugin:<plugin>:<server>` — `tool`, `input` with `${path}` substitution). Optional `if:`, `args:`, `timeout`, `async: true`.
- **[NEW] handler fields:** `statusMessage` (custom spinner text); `once` (skill frontmatter only); command hooks gain `asyncRewake` (background + exit-2 rewake; implies `async`) and `shell` (`bash`|`powershell`); HTTP hooks gain `allowedEnvVars` and `$VAR`/`${VAR}` header interpolation; prompt/agent hooks gain `model` (defaults to the fast model) plus `$ARGUMENTS` with `\$` escaping.
- **[NEW] matcher syntax:** exact, `|`- or `,`-separated, or unanchored JS regex.
- **[NEW] default timeouts:** 600 s (command/http/mcp_tool), 30 s (prompt), 60 s (agent); 30 s for `UserPromptSubmit`; 10 s for `MessageDisplay`; **1.5 s shared budget for `SessionEnd`** (raisable to 60 s).
- **Locations:** `~/.claude/settings.json`, `.claude/settings.json`, `.claude/settings.local.json`, managed settings, plugin `hooks/hooks.json`, skill or subagent frontmatter `hooks:`.
- **Decision protocol:** stdout JSON `{"hookSpecificOutput": {...}}` with `permissionDecision` (`allow|deny|ask`), `permissionDecisionReason`, `additionalContext`, `updatedInput`, **[NEW] `retry`** (PermissionDenied only); or exit codes; or HTTP response. **[NEW] universal fields** `continue`, `stopReason`, `systemMessage`, `terminalSequence`, `suppressOutput` (accepted, no effect); **[NEW] top-level `decision`** `allow|deny|`**`escalate`** + `reason`.
- **[NEW] common input fields:** `prompt_id` (v2.1.196+), an `effort` object (`low|medium|high|xhigh|max`), `agent_id`, `agent_type`; `permission_mode` now includes `auto` and `dontAsk`.
- **Env vars:** `CLAUDE_PROJECT_DIR`, **[NEW]** `CLAUDE_PLUGIN_DATA`, `CLAUDE_EFFORT`, `CLAUDE_CODE_REMOTE`, `CLAUDE_CODE_BRIDGE_SESSION_ID` (v2.1.199+), `CLAUDE_PLUGIN_OPTION_<KEY>`. **`OTEL_*` is stripped from hook subprocesses.**
- **Slash command:** `/hooks`; `disableAllHooks: true` kill switch; `allowManagedHooksOnly` (enterprise).
- **[NEW] guide topics:** re-inject context after compaction; audit configuration changes; reload environment on directory/file change; auto-approve specific permission prompts; combine results from multiple hooks; prompt-based hooks (**`continueOnBlock: true`**, response `"impossible": true`); agent-based hooks; HTTP hooks; hooks and permission modes; **Stop hooks are overridden after 8 consecutive blocks** (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`, `stop_hook_active` input). Documented **exec form vs shell form** (`"args": []` vs `sh -c` / Git Bash / PowerShell). `PreToolUse` fires before permission-mode checks in *every* mode including `dontAsk`/`bypassPermissions`; `allow` cannot override deny rules, `ask`-controlled connector tools, or `requiresUserInteraction` tools.

## 9. Sub-agents

> The background-agent CLI moved to the new **`agent-view`** page (§24) and the
> worktree keys to the new **`worktrees`** page.

- **What it does:** Spawn isolated Claude instances with their own system prompt, model, tools, permissions, and (optionally) worktree. Foreground or background.
- **Built-in subagents:** `Explore` (**[CHANGED]** now *inherits the main conversation model*, capped at Opus on the Claude API — no longer pinned to Haiku), `Plan`, `general-purpose`, `statusline-setup` (Sonnet), `claude-code-guide` (Haiku); **[NEW] `claude`** (catch-all; default for dispatched background sessions) and **[NEW] `fork`** (inherits the full parent session + tools).
- **Locations:** `~/.claude/agents/<name>.md`, `.claude/agents/<name>.md`. **[GONE] the `/agents` interactive editor was removed in v2.1.198** — `/agents` now just prints a reminder to edit `.claude/agents/` directly.
- **Frontmatter:** `name`, `description`, `tools`, `disallowedTools`, `model` (**[NEW]** accepts `fable`; default is `inherit`), `permissionMode` (**[NEW]** adds `auto`, `dontAsk`, `manual`), `maxTurns`, `skills`, `mcpServers`, `hooks`, `memory` (`user|project|local` → `~/.claude/agent-memory/<name>/`, `.claude/agent-memory/<name>/`, `.claude/agent-memory-local/<name>/`), `background: true`, `effort` (**[NEW]** adds `xhigh`, `max`), `isolation: worktree`, `color` (**[NEW]** enumerated: red, blue, green, yellow, purple, orange, pink, cyan), `initialPrompt`.
- **Invocation:** automatic delegation by description, `--agent <name>`, `--agents '<json>'`, the `Task` tool; **[NEW] `/subtask <task>`** (v2.1.212+, replaces `/fork` which existed v2.1.161–v2.1.211), **[NEW] `/tasks`**, **[NEW] `@"name (agent)"` / `@agent-<name>` / `@agent-<plugin>:<name>`** mentions, the `agent` settings key, and `--disallowedTools "Agent(Explore)"` / `permissions.deny: ["Agent(...)"]`.
- **Forking:** `claude --continue --fork-session`. **[NEW] fork mode is on by default in interactive sessions as of v2.1.232.**
- **[NEW] flag:** `--append-subagent-system-prompt` (v2.1.205+).
- **[NEW] env vars:** `CLAUDE_CODE_FORK_SUBAGENT`, `CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH` (default 3), `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` (default 20, v2.1.217+), `CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS`, `CLAUDE_CODE_SUBAGENT_MODEL`, `CLAUDE_CODE_DISABLE_AUTO_MEMORY`.
- **Worktree isolation:** `isolation: worktree` creates `.claude/worktrees/<name>`. **[NEW]** documented behavior: branch from the default branch or parent HEAD, auto-clean if unchanged, working-directory escape checks, git-redirect blocking. `baseRef` / `symlinkDirectories` / `sparsePaths` now live on the `worktrees` page.

## 10. MCP

- **What it does:** Bring external tools/data into the session via the Model Context Protocol.
- **CLI:** `claude mcp add`, `add-json`, `add-from-claude-desktop`, `list`, `get`, `remove`, `serve`; **[NEW] `login <name>`** (v2.1.186+), **[NEW] `logout <name>`**, **[NEW] `reset-project-choices`**. Flags `--transport/-t http|sse|stdio`, `--scope/-s local|project|user`, `--env/-e`, `--header/-H`; **[NEW] `--callback-port` (replaces `--mcp-oauth-port`)**, `--client-id`, `--client-secret` (masked prompt), `--no-browser`.
- **[NEW] transport: WebSocket (`type: "ws"`)** — `.mcp.json` / `add-json` only; accepts `url`, `headers`, `headersHelper`, `timeout`, `alwaysLoad`; no OAuth; `--transport` does not accept `ws`; WS servers don't appear in `claude mcp list`.
- **Slash command:** `/mcp` (status, auth, enable/disable).
- **Config files:** `.mcp.json` (project, committed), `~/.claude.json` (user/local), `--mcp-config`, `--strict-mcp-config`. **[NEW] server-config keys:** `alwaysLoad`, `headersHelper`, `oauth.clientId`, `oauth.callbackPort`, `oauth.authServerMetadataUrl`, `oauth.scopes`.
- **[NEW] `headersHelper` mechanism** — env `CLAUDE_CODE_MCP_SERVER_NAME`, `CLAUDE_CODE_MCP_SERVER_URL`, `CLAUDE_PLUGIN_ROOT`; 10 s timeout; auto re-run + single retry on 401/403.
- **Server env:** `CLAUDE_PROJECT_DIR` injected; `${CLAUDE_PROJECT_DIR}` and `${VAR:-default}` expansion.
- **[CHANGED] reserved names:** `workspace`, **plus** `claude-in-chrome`, `computer-use`, `Claude Preview`, `Claude Browser`.
- **Approvals:** per-server prompt; `enableAllProjectMcpServers`, `enabledMcpjsonServers`, `disabledMcpjsonServers`, `allowedMcpServers`, `deniedMcpServers`; **[NEW]** per-project `disabledMcpServers`/`enabledMcpServers` in `~/.claude.json` (distinct from the `…Mcpjson…` pair); **[NEW]** organization controls on connector tools (per-tool `ask`/`blocked`), `disableClaudeAiConnectors`, `ENABLE_CLAUDEAI_MCP_SERVERS=false`.
- **[NEW] scope precedence is now 5-tier:** local → project → user → plugin servers → claude.ai connectors (plugins/connectors match by endpoint, not name).
- **Reconnect:** HTTP/SSE 5× backoff (1 s doubling), startup 3 transient retries; stdio not reconnected. **[NEW] `MCP_CONNECTION_NONBLOCKING=0`** makes startup wait for background-connecting servers.
- **Limits / timeouts:** `MCP_TIMEOUT` (30 s default startup), per-server `timeout` (**[CHANGED]** values < 1000 are now *ignored* rather than clamped to a 1 s floor, per v2.1.162), 60 s per-request first-byte timer, `MCP_TOOL_TIMEOUT` (**[NEW]** default ~28 h), `MAX_MCP_OUTPUT_TOKENS` (**[NEW]** default 25 000; warn at 10 000), `tool.outputLimit`.
- **[NEW] per-tool `_meta` annotations:** `anthropic/maxResultSizeChars` (ceiling 500 000), `anthropic/requiresUserInteraction` (v2.1.199+), `anthropic/alwaysLoad`. **[NEW] root-level `anyOf`/`oneOf`/`allOf` schema flattening.**
- **[NEW] automatic backgrounding of long MCP tool calls** (> 2 min → background task, v2.1.212+): `CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS`, `CLAUDE_AUTO_BACKGROUND_TASKS`, `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`.
- **[NEW] idle timeout** (v2.1.187+): 5 min HTTP/SSE/WS/connectors, 30 min stdio; `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT`.
- **[NEW] remote tool-list discovery cache** (v2.1.221+): `cached Nh ago · connects on first use`; `MCP_DISCOVERY_CACHE=0`.
- **OAuth:** pre-configured creds via settings, RFC 8414-style metadata discovery overrides, scope restriction.
- **Elicitation:** server-initiated input mid-tool-call (via `Elicitation`/`ElicitationResult` hooks). **Resources:** `@server:resource`.
- **Tool Search:** built-in `ToolSearch` defers per-tool schemas until needed. **[CHANGED] `ENABLE_TOOL_SEARCH` now takes `true` | `false` | `auto` | `auto:N`** (default threshold 10 % of the context window); `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` forces it off (managed-settings override on v2.1.227+); `permissions.deny: ["ToolSearch"]`. `WaitForMcpServers` remains the non-tool-search fallback.
- **Channels:** `claude/channel` capability + `--channels plugin:<name>@<marketplace>`; now cross-linked to the new `channels` / `channels-reference` pages.
- **Plugin-bundled MCP:** `.mcp.json` at plugin root or inline in `plugin.json`, with `${CLAUDE_PLUGIN_ROOT}`. **Managed MCP** moved to the `managed-mcp` page.

## 11. Checkpointing

- **What it does:** Auto-snapshot file state per user prompt; allow restore/summarize from the rewind menu.
- **Key surfaces:** `/rewind`, Esc-Esc (empty input), per-prompt checkpoint list, options: restore code+conversation / restore conversation / restore code / summarize from here / summarize up to here; **[NEW] "Never mind"**. Code-restore options are hidden when no tracked file changes exist.
- **[NEW] cap:** file snapshots kept for the **100 most recent checkpoints**; each file's first snapshot is retained as the VS Code diff baseline.
- **[NEW] guide a summary** — type instructions at the `add context (optional)` row before pressing Enter.
- **[NEW] rewind past a cleared conversation** via the `/resume <session-id> (previous session)` entry (v2.1.191+).
- **Lifecycle:** persists across sessions (resumable), pruned with sessions after `cleanupPeriodDays` (default 30).
- **Limitations:** only file-tool edits tracked (not Bash `rm`/`mv`/`cp`); not version control; **[NEW] subagent edits are not restored** (except a foreground forked skill with `context: fork`, `background: false`); **[NEW] symlinked and hard-linked paths are not restored** — `Restored the code, but skipped N files`, with skipped paths logged by `/debug` to `~/.claude/debug/<session-id>.txt`.

## 12. Output styles

- **Built-in styles:** `Default`, `Proactive`, `Explanatory`, `Learning` (last inserts `TODO(human)` markers).
- **Custom styles:** markdown with frontmatter `name`, `description`, `keep-coding-instructions`, `force-for-plugin` at `~/.claude/output-styles/`, `.claude/output-styles/`, managed dir, or plugin `output-styles/`. **[NEW] nested project directories** load from every `.claude/output-styles/` between cwd and repo root, nearest wins.
- **Activation:** `/config → Output style`, or the `outputStyle` setting; takes effect after `/clear` or restart. **[GONE] the `/output-style` command was deprecated in v2.1.73 and removed in v2.1.91.** The terminal picker writes to `.claude/settings.local.json`.
- **[NEW]** output styles apply to the main conversation only — subagents are excluded; a **fork** is the exception.

## 13. Authentication (was `iam`)

- **Slash commands:** `/login`, `/logout`, `/status`. **[NEW]** `/status` gains a `Login` row (`Expired — log in again`, v2.1.210+) and a `Profile` row.
- **CLI:** `claude setup-token` (one-year OAuth token printed once). **[GONE from this page]** `claude auth login|logout|status` are no longer documented here (they remain in `cli-reference`).
- **Account types:** Claude Pro/Max/Team/Enterprise (OAuth), Claude Console (API billing), Bedrock/Vertex/Foundry via env.
- **[NEW] Claude apps gateway** — self-hosted; sign in via `/login`; `forceLoginMethod: "gateway"`; the gateway token is the session's only credential and **outranks all cloud providers**, sitting outside the numbered precedence list.
- **[NEW] Anthropic profiles + Workload Identity Federation** — `ANTHROPIC_PROFILE`, `ANTHROPIC_FEDERATION_RULE_ID` + `ANTHROPIC_ORGANIZATION_ID`, `ANTHROPIC_IDENTITY_TOKEN_FILE`, `active_config`; auth modes `oidc_federation` / `user_oauth`; config dir `~/.config/anthropic` or `%APPDATA%\Anthropic`.
- **[CHANGED] precedence — now 7 tiers:** cloud provider (`CLAUDE_CODE_USE_BEDROCK` / `_VERTEX` / **`_FOUNDRY`**) > `ANTHROPIC_AUTH_TOKEN` > `ANTHROPIC_API_KEY` > `apiKeyHelper` > `CLAUDE_CODE_OAUTH_TOKEN` > **[NEW] Anthropic profile / federation credentials** > subscription OAuth.
- **Credential storage:** macOS Keychain, Linux `~/.claude/.credentials.json` mode 0600, Windows `%USERPROFILE%\.claude\.credentials.json`; `CLAUDE_CONFIG_DIR` override.
- **Helper script:** `apiKeyHelper`, refresh `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` (default 5 min or on 401), 10 s slow-helper warning. **[NEW]** failures surface as `Your apiKeyHelper script is failing` within three attempts (v2.1.208+; previously a generic 401 after ~10 silent retries).
- **Org enforcement (managed):** `forceLoginMethod` — **[NEW]** applied on *every* login path as of v2.1.212 (previously terminal only); `forceLoginOrgUUID` blocks `ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN`/`apiKeyHelper` at startup (v2.1.146+) but not cloud providers or profiles; must be set in both device-managed and server-managed settings.
- **[NEW]** login-expiry warning `Your login expires in 3 days · run /login to renew` (v2.1.203+; 5 days before v2.1.217). `--bare` reads neither OAuth credentials nor `CLAUDE_CODE_OAUTH_TOKEN`.

## 14. Headless / programmatic (`headless`, retitled "Run Claude Code programmatically")

- **CLI/print-mode flags:** `-p`/`--print`, `--bare`, `--output-format text|json|stream-json`, `--input-format text|stream-json`, `--json-schema`, `--include-partial-messages`, `--include-hook-events`, `--max-turns`, `--max-budget-usd`, `--no-session-persistence`, `--replay-user-messages`, `--continue`, `--resume`, `--fallback-model`, `--permission-prompt-tool`; **[NEW]** `--forward-subagent-text`, `--append-system-prompt-file`, `--system-prompt`, `--plugin-dir`, `--plugin-url`, `--setting-sources`, `--safe-mode`, `--cloud`. **`--bg` is rejected with `-p`.**
- **Stream events:** `system/init`, `system/api_retry`, `system/plugin_install`, text deltas, `tool_use`, `tool_result`. **[NEW]** `system/init` gains a **`capabilities`** array (e.g. `interrupt_receipt_v1`, `interrupt_cancel_queued_v1`, v2.1.205+) plus **`mcp_servers`** / **`mcp_server_errors`** (v2.1.219+; skip categories `unknown_type`, `url_missing_type`, `invalid_config`, `reserved_name`). **[NEW] `hook_started`, `hook_progress`, `hook_response`** events precede `system/init`. `system/api_retry` now fully specified (`attempt`, `max_retries`, `retry_delay_ms`, `error_status`, `error` ∈ {`authentication_failed`, `oauth_org_not_allowed`, `billing_error`, `rate_limit`, `overloaded`, `invalid_request`, `model_not_found`, `server_error`, `max_output_tokens`, `unknown`}, `uuid`, `session_id`).
- **JSON result:** `result`, `session_id`, `structured_output` (with `--json-schema`), `total_cost_usd`, model breakdown.
- **[NEW] `--json-schema` hard-errors on an invalid schema** (`Error: --json-schema is not a valid JSON Schema`, v2.1.205+); `format` accepted but not enforced.
- **[NEW] lifecycle:** `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` caps the background-subagent wait at 10 min (v2.1.182+; `0` = unlimited); background Bash killed ~5 s after the final result (v2.1.163+); **SIGTERM** aborts the turn, kills the Bash process tree, runs `SessionEnd` hooks, and exits **143**; stream-drain wait capped at 30 s (v2.1.214+, was ~2 s).
- **[NEW]** `--resume <id>` finds sessions across projects on the machine (v2.1.223+). Slash commands usable in `-p`: `/model`, `/effort`, `/fast`, `/color`, `/rename`, `/mcp`, and `/config key=value` (v2.1.205+).
- **Bare mode:** skips hooks, skills, plugins, MCP, auto memory, CLAUDE.md; auth must come from `ANTHROPIC_API_KEY` or `apiKeyHelper`. Default tool palette = Bash + file read + file edit. Stdin cap 10 MB.
- **[GONE] the "from 2026-06-15 SDK/`-p` draws a separate Agent SDK monthly credit" pricing note** is no longer present on `headless` or `agent-sdk/overview`.

## 15. IDE integrations (`vs-code`, `jetbrains`)

- **Install:** `vscode:extension/anthropic.claude-code`, `cursor:extension/anthropic.claude-code`, Open VSX; JetBrains plugin via Marketplace (now its own page). **[NEW] prerequisite: VS Code 1.94.0+.**
- **Keybindings:** Spark icon, `Cmd+Esc`/`Ctrl+Esc`, `Cmd+Shift+P → Claude Code`, `Option+K`/`Alt+K` insert `@file#5-10`, `Ctrl+O` expand thinking blocks.
- **Prompt-box features:** `/` menu, permission-mode selector, context-usage indicator, extended-thinking toggle, Shift+Enter, `claudeCode.initialPermissionMode`. **[NEW] settings:** `claudeCode.useTerminal`, `claudeCode.disableLoginPrompt`.
- **Sessions panel:** history, fuzzy search, rename, remove, resume remote sessions from Claude.ai, multiple tabs/windows.
- **IDE MCP server**, walkthrough, `/terminal-setup`.
- **[NEW] sections:** resume cloud sessions from Claude.ai; **Check account and usage** (Day/Week toggle, v2.1.174+); run multiple conversations; **organize sessions into groups**; **manage plugins and marketplaces in-extension**; **automate browser tasks with Chrome**; launch a VS Code tab from other tools; **use git worktrees for parallel tasks**; use third-party providers; rewind with checkpoints; monitor background processes.

## 16. GitHub Actions

- **Setup:** `/install-github-app` (interactive) or manual. Repository: `anthropics/claude-code-action`. Examples use `actions/checkout@v6`, `anthropics/claude-code-action@v1`, models `claude-opus-4-8` / `claude-sonnet-5`.
- **Auth:** API key, OIDC for Bedrock/Vertex; **[NEW] workload identity federation via GitHub OIDC** — inputs `anthropic_federation_rule_id` (`fdrl_…`), `anthropic_organization_id`, `anthropic_service_account_id` (`svac_…`), `anthropic_workspace_id` (`wrkspc_…`); requires `id-token: write`.
- **[NEW] inputs:** `plugin_marketplaces`, `plugins`, `settings`, `allowed_non_write_users`, `allowed_bots`, `use_foundry`.
- **[NEW] access control "Who can trigger runs"** — write-access check + human-actor check.
- **[NEW]** the review workflow **posts to the PR** (v2.1.229+) via the `code-review` plugin skill and `--comment` (previously wrote only to the run log). `/install-github-app` offers "Update workflow file with latest version" and a "Skip for now" step (v2.1.187+).
- **[NEW] sections:** Uninstall; GitHub App permissions table (Actions, Checks, Contents, Discussions, Issues, Members, Metadata, Pull requests, Repository hooks, Statuses, Workflows).
- Beta→v1 migration retained (`mode` removed, `direct_prompt`→`prompt`, `custom_instructions`→`--append-system-prompt`). Cloud-provider setup moved to `github-actions-cloud-providers`; GitHub Enterprise Server and GitLab CI/CD are separate pages.

## 17. Dev containers

- **Install:** `ghcr.io/anthropics/devcontainer-features/claude-code:1.0` feature in `devcontainer.json`.
- **Persistence:** volume mount at `/home/<user>/.claude` (`source=claude-code-config-${devcontainerId}`). **[NEW] explicit:** mounting `~/.claude` alone is insufficient — you must **also set `CLAUDE_CONFIG_DIR`** to the same path so `~/.claude.json` lands in the volume. `~/.claude` survives stop/start but not rebuild.
- **Policy:** `/etc/claude-code/managed-settings.json` baked via Dockerfile `COPY`; `containerEnv`; reference firewall script (`init-firewall.sh`); `runArgs` for `NET_ADMIN`/`NET_RAW`. **[NEW] `permissions.disableBypassPermissionsMode: "disable"`** to block the bypass flag, with auto mode as the softer alternative.
- **[NEW]** `DISABLE_AUTOUPDATER` in `containerEnv`; pin a version with `npm install -g @anthropic-ai/claude-code@X.Y.Z`; `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` also kills Remote Control's feature-flag fetch; Node.js auto-install with a `ghcr.io/devcontainers/features/node:1` fallback; Codespaces secrets for `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN`.
- **Unattended:** `--dangerously-skip-permissions` (rejected when launched as root).

## 18. Troubleshooting

> Now a **router** page. Content split to `troubleshoot-install`,
> `debug-your-config`, the new **`errors`** reference, `vs-code`, `jetbrains`.

- **Retained surfaces:** `/doctor`, `claude doctor` from the shell, `/heapdump`, `/compact`, `/feedback`, `/mcp`, `USE_BUILTIN_RIPGREP=0`.
- **[NEW] `/heapdump` writes two files** to `~/Desktop` — `<session-id>.heapsnapshot` **and `<session-id>-diagnostics.json`** — plus an in-conversation summary (RSS, JS heap, array buffers, unaccounted native memory, leak indicators). Falls back to the home dir on Linux.
- **[NEW] `claude --safe-mode`** for isolating a plugin/MCP/hook.
- **[NEW]** large tables cut off at 200 rows (`… N more rows not shown`, v2.1.208+); `/copy` gets all rows.
- **[NEW]** autocompact-thrashing error: `Autocompact is thrashing: the context refilled to the limit…`.
- **[NEW]** `/terminal-setup` documented as the fix for garbled glyphs (sets `terminal.integrated.gpuAcceleration: "off"`).

## 19. Costs

- **Slash commands:** `/usage`, `/compact`, `/clear`, `/rewind`, `/context`, `/effort`, `/mcp`, `/model`, `/fast`; **[NEW] `/insights`** (HTML report to `~/.claude/usage-data/report.html`, up to 200 sessions/run) and **[NEW] `/usage-credits`**.
- **[NEW] `/usage` detail:** plan-usage breakdown with **attribution** (skills, subagents, plugins, individual MCP servers) and **behavior flags** (flagged at ≥ 10 % of recent usage); `d`/`w` toggles 24 h vs 7 d; `Showing last-known usage` fallback with `r` to retry (v2.1.208+). **[CHANGED] totals now reset on `/clear`** (v2.1.211+).
- **Settings / env:** `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`, `MAX_THINKING_TOKENS`, `cleanupPeriodDays`; **[NEW]** `ENABLE_PROMPT_CACHING_1H=1`, `crossSessionInbound: "hold"`.
- **[NEW] section "Why usage climbs in a long session"** — long context, cache misses, scheduled tasks, cross-session messages, agent teammates, compaction.
- **[NEW] quantitative facts:** ~$13/dev/active-day, $150–250/dev/month; **agent teams use ~7× more tokens** in plan mode; background token usage < $0.04/session. Teams/Enterprise seat allowance on rolling 5-hour + weekly windows, shared with Claude chat and Cowork; Standard vs Premium seat tiers; Enterprise Analytics API with the `read:analytics` scope.
- **[NEW] "When a developer asks about a limit"** distinguishes session/weekly limit, gateway spend limit, auto-compact warning, and high API spend. Note: "Disabling thinking is not available on Fable 5."
- **Workspace limits:** Console spending caps, organization rate-limit TPM/RPM table. Reduction patterns retained. Bedrock/Vertex/Foundry: no metrics emitted.

## 20. Monitoring usage (OpenTelemetry)

- **Env vars:** `CLAUDE_CODE_ENABLE_TELEMETRY=1`; `OTEL_METRICS_EXPORTER` (`otlp|prometheus|console|none`); `OTEL_LOGS_EXPORTER`; **[NEW] `OTEL_TRACES_EXPORTER`**; `OTEL_EXPORTER_OTLP_PROTOCOL|ENDPOINT|HEADERS` plus signal-specific `OTEL_EXPORTER_OTLP_{METRICS,LOGS,TRACES}_{PROTOCOL,ENDPOINT,HEADERS,CLIENT_KEY,CLIENT_CERTIFICATE}`; `OTEL_METRIC_EXPORT_INTERVAL` (60 s), `OTEL_LOGS_EXPORT_INTERVAL` (5 s), **[NEW] `OTEL_TRACES_EXPORT_INTERVAL`**; content controls `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_TOOL_DETAILS`, `OTEL_LOG_TOOL_CONTENT`, `OTEL_LOG_RAW_API_BODIES`, **[NEW] `OTEL_LOG_ASSISTANT_RESPONSES`** and **`CLAUDE_CODE_OTEL_CONTENT_MAX_LENGTH`** (both v2.1.214+); cardinality `OTEL_METRICS_INCLUDE_SESSION_ID`/`_VERSION`/`_ACCOUNT_UUID` **[NEW] `_ENTRYPOINT`, `_RESOURCE_ATTRIBUTES`**; **[NEW]** `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE`, `OTEL_RESOURCE_ATTRIBUTES`, `CLAUDE_CODE_OTEL_HEADERS_HELPER_DEBOUNCE_MS`, `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA` / `ENABLE_ENHANCED_TELEMETRY_BETA`; `otelHeadersHelper` setting; mTLS via `CLAUDE_CODE_CLIENT_CERT` / `_KEY` / `_KEY_PASSPHRASE` / `NODE_EXTRA_CA_CERTS`.
- **[NEW] full distributed tracing** (absent from the 2026-05-24 capture): `CLAUDE_CODE_PROPAGATE_TRACEPARENT`, `TRACEPARENT`, `TRACESTATE`; span types **Interaction, LLM Request, Tool, Tool Blocked On User, Tool Execution, Hook (beta)**.
- **Standard attributes:** `session.id`, `app.version`, `organization.id`, `user.account_uuid`, `user.account_id`, `user.id`, `user.email`, `terminal.type`, plus event-only `prompt.id`, `workspace.host_paths`; **[NEW]** `app.entrypoint`, `user.groups`, `identity.source`, `message.uuid` (v2.1.214+), `client_request_id` (v2.1.214+), `workflow.run_id` / `workflow.name` (v2.1.202+), `event.name`, `event.timestamp`, `event.sequence`.
- **[CHANGED] metrics now carry the `claude_code.` prefix explicitly** and number exactly eight — no additions, no removals: `claude_code.session.count`, `claude_code.lines_of_code.count`, `claude_code.pull_request.count`, `claude_code.commit.count`, `claude_code.cost.usage`, `claude_code.token.usage`, `claude_code.code_edit_tool.decision`, `claude_code.active_time.total`. **[NEW]** cost/token/api_request counters carry `speed`, `effort`, `query_source`, `agent.name`, `skill.name`, `plugin.name`, `marketplace.name`, `mcp_server.name`, `mcp_tool.name`.
- **[CHANGED] events are now namespaced `claude_code.*`.** Retained: `user_prompt`, `tool_result`, `api_request`, `api_error`, `api_request_body`, `api_response_body`, `tool_decision`, `permission_mode_changed`, `auth`, `mcp_server_connection`, `internal_error`, `plugin_installed`, `plugin_loaded`. **[NEW]** `claude_code.assistant_response` (v2.1.193+), `claude_code.api_refusal`.
- **[GONE] events** the 2026-05-24 capture listed that are absent now: `skill_activated`, `at_mention`, `api_retries_exhausted`, `hook_registered`, `hook_execution_start`, `hook_execution_complete`, `hook_plugin_metrics`, `compaction`, `feedback_survey`. Hook observability appears to have migrated to the beta **Hook spans** (`hook_event`, `hook_name`, `num_hooks`, `hook_definitions`, `num_success`, `num_blocking`, …).

## 21. Data usage

- **Policies:** consumer (Free/Pro/Max) opt-in toggle for training; commercial (Team/Enterprise/API) no training without explicit opt-in.
- **Retention:** 30 days default; 5 years if a consumer opts in; ZDR available for Enterprise.
- **[CHANGED] error reporting is now ON by default** for Pro/Max sign-ins on v2.1.198+ connecting directly to the Claude API without ZDR/HIPAA. `DISABLE_ERROR_REPORTING=1` opts out; it does *not* disable Remote Control's feature-flag fetch, whereas `DISABLE_TELEMETRY` and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` do.
- **Feedback:** `/feedback` — **[NEW] `/bug` and `/share`** report through the same path; transcripts retained 5 years; **[NEW]** history-scope selector (current session / project 24 h / project 7 d). Session-quality surveys: shared transcripts retained up to 6 months. Local archive `~/.claude/feedback-bundles/` on Bedrock/Vertex/Foundry — **[NEW]** and on signed-in Claude apps gateway sessions. `feedbackSurveyRate` controls frequency.
- **[NEW] provider column: Claude Platform on AWS** (`CLAUDE_CODE_USE_ANTHROPIC_AWS`), alongside Microsoft Foundry (`CLAUDE_CODE_USE_FOUNDRY`).
- **Telemetry opt-outs:** `DISABLE_TELEMETRY`, `DISABLE_ERROR_REPORTING`, `DISABLE_FEEDBACK_COMMAND`, `CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `CLAUDE_CODE_ENABLE_FEEDBACK_SURVEY_FOR_OTEL`, `DO_NOT_TRACK`; **[NEW] `CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL`**.
- **Encryption at rest:** AES-256 per provider table; `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST`.
- **WebFetch domain safety:** hostname-only check against `api.anthropic.com`; **[NEW]** pass cached 5 min, blocked/failed re-checked next request; disable with `skipWebFetchPreflight: true`.
- **[NEW]** cloud-execution data-flow section for Claude Code on the web + self-hosted environments; per-session delete.

## 22. Background bash

- **What it does:** Run long-running Bash commands without blocking the turn.
- **Surfaces:** Ctrl+B to background a running Bash, automatic stderr note when output > 5 GB, output retrievable via `Read`, IDs unique per task, auto-cleanup at exit.
- **[NEW]** memory-pressure reaping after 30 min idle on macOS/Linux (v2.1.193+); subagent-owned background commands terminated after 60 min.
- **Config:** `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`, `CLAUDE_CODE_DISABLE_BG_SHELL_PRESSURE_REAP=1`, `CLAUDE_SUBAGENT_BG_SHELL_MAX_MS`.

## 23. Agent SDK (`agent-sdk/overview`)

- Now a short landing page fronting a ~30-page `agent-sdk/*` tree.
- **Positioning table:** Agent SDK vs Claude Code CLI vs Client SDK vs **Managed Agents** (hosted REST API — new).
- Python and TypeScript only; other languages should run the CLI with `-p --output-format json`.
- Capability table linking hooks, subagents, MCP, permissions, sessions, skills/commands/memory, plugins.
- **[NEW] branding guidelines** — "Claude Agent" / "Claude" / "{YourAgentName} Powered by Claude" allowed; "Claude Code" / "Claude Code Agent" and Claude Code ASCII art **not permitted**. Third parties may not offer claude.ai login or rate limits for SDK-built products without approval.
- Repos: `claude-agent-sdk-typescript`, `claude-agent-sdk-python`, `claude-agent-sdk-demos`.

## 24. Agent View (new page — background-agent control plane)

Absorbs the background-agent CLI that used to live on `sub-agents`, and adds a
great deal. **Research preview.**

- **CLI:** `claude agents [--json|--all|--cwd]`, `claude attach <id>`, `claude logs <id>`, `claude stop <id>` / `claude kill <id>`, `claude respawn <id> [--all]`, `claude rm <id>`, `claude daemon status`, `claude daemon stop --any [--keep-workers]`, `claude --bg [--exec] [--name]`.
- **Slash:** `/bg`, `/background`, `/loop`.
- **Keybindings:** `Ctrl+S`, `Ctrl+T`, `Ctrl+R`, `Ctrl+G`, `Ctrl+X`, `Alt+1..9`.
- **Filters:** `a:`, `s:`, `#<num>`, `@repo`, `! <cmd>`.
- **Env:** `CLAUDE_CODE_DISABLE_AGENT_VIEW`, `CLAUDE_CODE_DISABLE_BG_EXIT_HANDOFF`, `CLAUDE_DISABLE_ADOPT`, `CLAUDE_JOB_DIR`.
- **Settings:** `disableAgentView`, `leftArrowOpensAgents`, `worktree.bgIsolation`.
- **Paths:** `~/.claude/daemon.log`, `~/.claude/daemon/roster.json`, `~/.claude/daemon.lock`, `~/.claude/jobs/<id>/`.

## 25. New doc pages since 2026-05-24

The doc site grew from ~24 pages to 100+. Pages that now exist and did not at
the last capture, grouped by theme — listed so the next re-baseline knows what
to sweep:

- **Agent orchestration:** `agent-view`, `agent-teams`, `cross-session-messaging`, `workflows`, `worktrees`, `sessions`, `ultrareview`, `goal`.
- **Reference:** `commands`, `tools-reference`, `env-vars`, `keybindings`, `glossary`, `claude-directory`, `permission-modes`, `errors`, `how-claude-code-works`, `features-overview`, `context-window`, `prompt-caching`, `model-config`, `auto-mode-config`, `fast-mode`, `advisor`, `fullscreen`.
- **MCP / channels:** `mcp-quickstart`, `managed-mcp`, `channels`, `channels-reference`.
- **Troubleshooting split:** `troubleshoot-install`, `debug-your-config`.
- **IDE / desktop / mobile:** `vs-code`, `jetbrains`, `chrome`, `computer-use`, `desktop*` (7 pages), `mobile`, `artifacts`.
- **Cloud / remote:** `remote-control`, `claude-code-on-the-web`, `web-quickstart`, `routines`, `scheduled-tasks`, `deep-links`, `slack` / `claude-tag`.
- **Enterprise / hosting:** `self-hosted-environments*` (7 pages), `claude-apps-gateway*` (6 pages), `llm-gateway*` (4 pages), `server-managed-settings`, `microsoft-foundry`, `claude-platform-on-aws`, `github-actions-cloud-providers`, `github-enterprise-server`, `gitlab-ci-cd`, `zero-data-retention`, `analytics`.
- **Security / review:** `code-review`, `security-guidance` / `claude-security`, `sandboxing` / `sandbox-environments`.
- **Plugins:** `plugin-*` (4 pages).
- **Other:** `voice-dictation`, `communications-kit`, `champion-kit`, and a weekly **`whats-new/2026-wNN`** series (through `2026-w32` at capture).

---

## Canonical built-in tool surface

Anthropic now publishes a **`tools-reference`** page (new since the last
capture); the names below are cross-checked against it and against CLI flags,
settings examples, sub-agent frontmatter, and hooks docs:

- **File I/O:** `Read`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit`, `Glob`, `Grep`.
- **Shell:** `Bash`, plus a `PowerShell` tool gated by `CLAUDE_CODE_USE_POWERSHELL_TOOL=1` and `defaultShell: "powershell"`.
- **Web:** `WebFetch` (with `WebFetch(domain:...)` rules and the domain-safety preflight), `WebSearch`.
- **Orchestration:** `Task`/`TaskCreate`, `Skill`, `EnterWorktree`/`ExitWorktree`, `WaitForMcpServers`, `ToolSearch`, **[NEW] `EndConversation`** (referenced by the skills page's `disallowed-tools` rule).
- **MCP:** any tool exposed by a connected server, permissioned as `mcp__<server>__<tool>`.

## Slash-command surface

Now authoritatively listed on the new **`commands`** page. Commands mentioned
across the corpus at this capture: `/agents` (**editor removed v2.1.198** —
now a pointer), `/background`, `/bg`, `/btw`, `/bug`, `/clear`, `/code-review`
(alias `/review`), `/color`, `/compact`, `/config`, `/context`, `/copy`,
`/debug`, `/desktop`, `/doctor` (**now a bundled skill**), `/effort`,
`/exit`, `/fast`, `/feedback`, `/focus`, `/heapdump`, `/help`, `/hooks`,
`/import`, `/init`, `/insights`, `/install-github-app`, `/login`, `/logout`,
`/loop`, `/mcp`, `/memory`, `/model`, `/plugin` (alias `/plugins`), `/recap`,
`/reload-plugins`, `/rename`, `/resume`, `/rewind`, `/run`,
`/run-skill-generator`, `/schedule`, `/security-review`, `/share`, `/skills`,
`/statusline`, `/status`, `/subtask`, `/tasks`, `/terminal-setup`, `/theme`,
`/tui`, `/ultrareview`, `/usage`, `/usage-credits`, `/verify`, `/voice`.
**[GONE]** `/output-style` (removed v2.1.91), `/fork` (v2.1.161–v2.1.211,
replaced by `/subtask`).

## Surfaces seen but not slotted into a single area

- **Plugins:** complete extension package (skills, hooks, agents, output-styles, MCP servers, statusline). Settings expose `enabledPlugins`, marketplace allow/blocklists, `strictPluginOnlyCustomization`, `pluginTrustMessage`, and the new `extraKnownMarketplaces` / `disableCommandPluginSources` / `disableSideloadFlags` / `pluginSuggestionMarketplaces`. Surfaces: `/plugin`, `/reload-plugins`, `claude plugin`, `--plugin-dir`, `--plugin-url`, marketplaces, four dedicated `plugin-*` doc pages.
- **Status line:** `statusLine` setting, `/statusline` slash, `statusline-setup` subagent, **[NEW] `subagentStatusLine`**.
- **Auto mode:** classifier-driven rules (`autoMode.environment/allow/soft_deny/hard_deny`, `$defaults`). `claude auto-mode defaults|config|`**`reset`**. `disableAutoMode`, **[NEW] `autoMode.classifyAllShell`**, and a dedicated `auto-mode-config` page.
- **Permission modes (full list):** `default` (**[NEW]** UI-labeled *Manual*, with `manual` as a CLI alias), `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`. Cycled with Shift+Tab. Dedicated `permission-modes` page.
- **Worktrees:** `--worktree` (**[NEW]** accepts a PR/MR URL or `#N`), `EnterWorktree` tool, sub-agent `isolation: worktree`, `.worktreeinclude`, `worktree.*` settings, dedicated `worktrees` page.
- **Channels (research preview):** MCP-driven push notifications, `--channels`, `--dangerously-load-development-channels`, `channelsEnabled`, `allowedChannelPlugins`; two dedicated pages.
- **Remote Control:** `claude remote-control`, `--remote-control`/`--rc`, `--remote-control-session-name-prefix`, `disableRemoteControl`, `remoteControlAtStartup`; dedicated page.
- **Routines / scheduling:** `/schedule`, Desktop scheduled tasks, `/loop`; dedicated `routines` and `scheduled-tasks` pages.
- **Teleport:** `claude --teleport` moves a web session into the terminal.
- **Deep links:** `claude-cli://` protocol handler; `disableDeepLinkRegistration`; dedicated page.
- **Fast mode:** Option+O, `/fast`, `fastModePerSessionOptIn`, `fastMode`; dedicated `fast-mode` page.
- **[NEW] Advisor:** server-side advisor tool, `--advisor`, `advisorModel`; dedicated page.
- **[NEW] Agent teams:** `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`, `teammateMode` (`in-process`/`auto`/`tmux`/`iterm2`), `teammateDefaultModel`, `TeammateIdle` hook; dedicated page. ~7× token usage in plan mode.
- **[NEW] Cross-session messaging:** `@`-mention live sessions on the machine (v2.1.232+), `crossSessionInbound`, `isolatePeerMachines`; dedicated page.
- **[NEW] Claude apps gateway / self-hosted environments:** `claude gateway`, `claude self-hosted-runner`, `--environment ccpool_…`, `--ref`, `forceLoginGatewayUrl`, `remote.defaultEnvironmentId`; 13 dedicated pages.
- **[NEW] Cowork** — named on Overview as a distinct surface sharing the seat allowance.
- **Sandboxing model:** macOS Seatbelt + Linux bubblewrap + WSL2 + macOS Mach lookup allowlist; per-OS knobs in `sandbox.*`; **[NEW]** `sandbox.credentials` injection, `processWrapper`, `network.strictAllowlist`/`tlsTerminate`; dedicated `sandboxing` / `sandbox-environments` pages.
- **Onboarding/auto-update:** native installer auto-updates by default; `autoUpdatesChannel`, `minimumVersion`, **[NEW]** managed `requiredMinimumVersion`/`requiredMaximumVersion`, `DISABLE_AUTOUPDATER`.
- **Heap dump / debug:** `/heapdump` (**[NEW]** now two files), `--debug` (**[NEW]** category filter), `--debug-file`, `CLAUDE_CODE_DEBUG_LOGS_DIR`, **[NEW] `--safe-mode`**.

---

## Source pages (fetched 2026-08-15)

All pages live at `https://docs.claude.com/en/docs/claude-code/<slug>`.
Markdown export at `https://code.claude.com/docs/en/<slug>.md`; canonical
machine-readable index at `https://code.claude.com/docs/llms.txt`.

| Page | Status | Notes |
|---|---|---|
| overview | ✓ | surfaces restructured into tabs |
| quickstart | ✓ | |
| cli-reference | ✓ | exhaustive flag table; 8 new subcommands, ~15 new flags |
| interactive-mode | ✓ | full key map; fullscreen renderer now first-class |
| slash-commands | ✓ | **byte-identical to `skills`** |
| skills | ✓ | Agent Skills open standard |
| commands | ✓ | **new** — authoritative built-in command list |
| settings | ✓ | ~140 top-level keys (was ~80) |
| memory | ✓ | import depth 5 → 4; `/init` sources changed |
| hooks | ✓ | 31 events; `mcp` → `mcp_tool` |
| hooks-guide | ✓ | tutorial companion, much expanded |
| sub-agents | ✓ | `/agents` editor removed |
| agent-view | ✓ | **new** — background-agent control plane |
| worktrees | ✓ | **new** — absorbed `worktree.*` keys |
| mcp | ✓ | WebSocket transport; 5-tier scope precedence |
| managed-mcp | ✓ | **new** |
| checkpointing | ✓ | 100-checkpoint snapshot cap |
| output-styles | ✓ | `/output-style` removed |
| authentication | ✓ | **renamed from `iam`**; 7-tier precedence |
| agent-sdk/overview | ✓ | **renamed from `sdk/overview`**; ~30-page tree |
| vs-code | ✓ | **split from `ide-integrations`** |
| jetbrains | ✓ | **split from `ide-integrations`** |
| github-actions | ✓ | OIDC federation inputs |
| devcontainer | ✓ | `CLAUDE_CONFIG_DIR` requirement clarified |
| headless | ✓ | retitled "Run Claude Code programmatically" |
| troubleshooting | ✓ | now a router page |
| costs | ✓ | `/insights`, `/usage-credits` |
| monitoring-usage | ✓ | full tracing added; 9 events dropped |
| data-usage | ✓ | error reporting now on by default |
| tools-reference | ✓ | **new** — first published tool list |
| env-vars | ✓ | **new** |
| keybindings | ✓ | **new** — rebindable actions |
| changelog | ✓ | **new** — source of the v2.1.233 currency marker |
