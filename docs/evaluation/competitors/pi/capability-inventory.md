# Pi documented-capability inventory

> **Static snapshot — captured 2026-08-15.**
>
> Structured snapshot of **Pi**'s documented surface, captured from the canonical
> docs at `https://pi.dev/docs/latest/*` and the project repo
> `github.com/earendil-works/pi`. This is the *source* feeding
> [`parity-gap-matrix.md`](parity-gap-matrix.md). It is intentionally a
> point-in-time capture, not a live mirror.
>
> **Scope note — toolkit vs. coding-agent CLI.** Pi is **not** purely a terminal
> coding agent. Its own GitHub description is *"AI agent toolkit: unified LLM API,
> agent loop, TUI, coding agent CLI"* — a monorepo of ten packages, only one of
> which is the head-to-head competitor. Applying the precedent set for
> [Antigravity](../antigravity/capability-inventory.md):
> - **Head-to-head with caliban:** `packages/coding-agent`
>   (`@earendil-works/pi-coding-agent`, the `pi` binary) — the TUI, CLI, tools,
>   skills/extensions/packages model, auth, sessions, and config. These rows are
>   genuine apples-to-apples parity.
> - **Adjacent toolkit surface (noted, not scored):** `pi-ai` (unified LLM API),
>   `pi-agent-core` (agent loop), `pi-tui` (terminal UI library), `pi-telemetry`.
>   These are independently consumable libraries competing with LLM SDKs, not with
>   caliban. They are inventoried in §17 and tagged **(tk)** in the matrix, which
>   does not score them as parity gaps.
> - **Experimental / out of scope:** `pi-protocol`, `pi-server`, `pi-client` (a
>   CBOR remote-session transport) are marked experimental upstream and are
>   **not referenced anywhere in the coding-agent docs** — see §16 and the
>   uncertainties list.
>
> **Live-docs status:** every canonical page below returned **HTTP 200** this
> pass, as did all 30 `packages/coding-agent/docs/*.md` files read raw from
> GitHub. Facts here are read straight off primary sources — `pi.dev`, the repo,
> the GitHub API, and the npm registry API. **No third-party write-up was used**:
> several circulating blog summaries carry a stale "62.4k stars" figure and
> unverified Hacker News / benchmark claims. Pi publishes **no benchmark numbers
> at all** (see §18), so any benchmark claim about it is unsourced.
>
> **Repo lineage (confirmed):** `badlogic/pi-mono` **301-redirects** to
> `earendil-works/pi` — the same project, renamed, not a fork. The npm scope moved
> with it: the legacy `@mariozechner/pi-coding-agent` (created 2025-11-12, last at
> 0.73.1) was superseded by `@earendil-works/pi-coding-agent` (first published
> 2026-05-07). Author **Mario Zechner (badlogic)**, published under **Earendil
> Inc.** License **MIT**, language **TypeScript**. ⚠ Many in-doc links still point
> at the old `pi-mono` path; they redirect, but signal the rename was not fully
> propagated.
>
> **Re-baseline cadence:** refresh manually before each parity-prioritization
> review. When refreshing, re-fetch the upstream docs, update the sections below,
> bump the snapshot date in this header, and propagate any new rows into
> `parity-gap-matrix.md` in the same commit. Pi ships **~one release every 2.5
> days** (§18), so this snapshot ages faster than any sibling inventory — treat a
> months-old capture as stale.
>
> Conventions: *surfaces* = user-visible primitives; "Config = X" lines name the
> canonical configuration mechanism. **(tk)** marks broader-toolkit surface that is
> adjacent, not head-to-head. Items still carrying upstream uncertainty are marked
> **⚠ verify** (see §19).

## 1. Overview / surfaces

- **What it is:** *"A minimal terminal coding harness… designed to stay small at
  the core while being extended through TypeScript extensions, skills, prompt
  templates, themes, and pi packages"* (`docs/index.md`). Homepage tagline:
  *"Pi is a minimal agent harness. Adapt Pi to your workflows, not the other way
  around."*
- **Key surfaces:** interactive TUI (`pi`), non-interactive print mode (`pi -p`),
  a JSONL event stream (`--mode json`), a line-delimited **RPC** mode
  (`--mode rpc`), an in-package **SDK**, and an HTML session exporter
  (`pi --export`). No web UI, no IDE extension, no server daemon.
- **Runtime:** TypeScript on Node **≥22.19.0**; standalone Bun-compiled binaries
  for 6 platform targets.
- **Repo / docs:** `pi.dev` (canonical docs, mirrored from
  `packages/coding-agent/docs/`); repo `github.com/earendil-works/pi` (**MIT**).
  There is **no top-level `docs/` directory** — all docs live under
  `packages/coding-agent/docs/`.

### Package layout (all at 0.84.2)

| Package | npm name | Role | Slice |
|---|---|---|---|
| `coding-agent` | `@earendil-works/pi-coding-agent` | Coding-agent CLI (`bin: pi`) + SDK | **head-to-head** |
| `ai` | `@earendil-works/pi-ai` | Unified multi-provider LLM API (`bin: pi-ai`) | toolkit **(tk)** |
| `agent` | `@earendil-works/pi-agent-core` | Agent runtime / loop, tool calling, state | toolkit **(tk)** |
| `tui` | `@earendil-works/pi-tui` | Terminal-UI library, differential rendering | toolkit **(tk)** |
| `telemetry` | `@earendil-works/pi-telemetry` | Vendor-neutral telemetry contracts | toolkit **(tk)** |
| `protocol` / `server` / `client` | `…/pi-protocol`, `-server`, `-client` | CBOR remote-session transport — **experimental** | out of scope |
| `session-backends/sqlite-node` | `…/pi-session-backend-sqlite-node` | Node SQLite session backend | toolkit **(tk)** |
| `evals` | `@earendil-works/pi-evals` | Model-backed behavioral evals | unpublished (`private`) |

`npm i -g @earendil-works/pi-coding-agent` installs only the head-to-head slice;
it pulls `pi-agent-core`, `pi-ai`, `pi-tui`, `pi-client`, `pi-protocol` as runtime
deps. **The SDK ships inside the CLI package** — there is no separate SDK package.

## 2. Install & distribution

- **Methods:** npm (primary, `npm i -g --ignore-scripts @earendil-works/pi-coding-agent`),
  `curl -fsSL https://pi.dev/install.sh | sh`, PowerShell `irm https://pi.dev/install.ps1 | iex`,
  pnpm, bun, and **standalone binaries** on GitHub Releases.
- **Binary assets (v0.84.2):** `pi-{darwin,linux}-{arm64,x64}.tar.gz`,
  `pi-windows-{arm64,x64}.zip`, plus a source tarball and `SHA256SUMS`. Builds are
  **reproducible** from the covered source tarball.
- **Platforms:** macOS, Linux, Windows (**requires a bash shell** — Git Bash /
  Cygwin / MSYS2 / WSL), and **Android via Termux** (dedicated doc page).
- **Self-update:** `pi update --self` (`--force` reinstalls); `pi update --all`,
  `--extensions`, `--models`. Version check hits `pi.dev/api/latest-version`;
  anonymous install telemetry pings `pi.dev/api/report-install`
  (`enableInstallTelemetry`, default `true`). Disable with `PI_SKIP_VERSION_CHECK`,
  `--offline`, or `PI_OFFLINE=1`.
- ⚠ **No Homebrew formula** is documented anywhere.
- **Supply-chain hardening (documented as a product feature):** exact-pinned direct
  deps, `.npmrc` `save-exact=true` + `min-release-age=2`, a published
  `npm-shrinkwrap.json`, a lifecycle-script allowlist, `--ignore-scripts`
  everywhere *including* `pi update --self`, and scheduled `npm audit` +
  `npm audit signatures`.

## 3. CLI reference

**Invocation:** `pi [options] [@files...] [messages...]`. Authoritative help text
lives in `packages/coding-agent/src/cli/args.ts`.

- **Subcommands:** `install <source> [-l]`, `remove` / `uninstall`, `update
  [source|self|pi]` (`--all`, `--extensions`, `--models`, `--self [--force]`,
  `--extension <src>`), `list`, `config [-l]` (TUI to enable/disable package
  resources; Tab switches scope), `auth <command>`.
- **`pi auth`:** `check`, `print-api-key --provider <p>`,
  `print-bearer-token --provider <p>` — deliberately exposes resolved credentials
  to external clients.
- **Modes:** default TUI · `-p`/`--print` non-interactive · `--mode text|json|rpc`
  (`text` default; `json` = JSONL event stream; `rpc` = JSONL RPC over
  stdin/stdout) · `--export <in> [out]` → HTML.
  ⚠ **There is no `--output-format` and no bare `--json` flag** — format is
  selected only via `--mode`. Print mode merges piped stdin into the prompt.
- **Model flags:** `--provider`, `--model <pattern>` (accepts `provider/id` and a
  `:<thinking>` suffix, e.g. `sonnet:high`), `--api-key`, `--thinking
  <off|minimal|low|medium|high|xhigh|max>`, `--models <patterns>` (globs + fuzzy),
  `--list-models [search]`.
- **Session flags:** `-c`/`--continue`, `-r`/`--resume`, `--session <path|id>`
  (partial UUID ok), `--session-id <id>`, `--fork <path|id>`, `--session-dir`,
  `--no-session`, `-n`/`--name`.
- **Tool flags:** `-t`/`--tools <list>`, `-xt`/`--exclude-tools`,
  `-nbt`/`--no-builtin-tools`, `-nt`/`--no-tools`.
- **Resource flags:** `-e`/`--extension <src>` (repeatable), `-ne`/`--no-extensions`,
  `--skill <path>`, `-ns`/`--no-skills`, `--prompt-template <path>`,
  `-np`/`--no-prompt-templates`, `--theme <path>`, `--no-themes`,
  `--use-theme <name>`, `-nc`/`--no-context-files`.
- **Other:** `--system-prompt`, `--append-system-prompt`, `--tui-mode
  <regular|fullscreen>`, `--verbose`, `-a`/`--approve`, `-na`/`--no-approve`,
  `--offline`. `@file` prefixes attach files (including images).
  **Unknown `--flags` are collected and passed to extensions** (`pi.registerFlag`).
- **Exit codes** ⚠ undocumented; from `src/modes/print-mode.ts`: `0` normal, `1` on
  `error`/`aborted` stop reason or thrown exception, `129` SIGHUP, `143` SIGTERM.

## 4. Interactive TUI

- **Layout:** startup header (shortcuts, loaded context files, prompt templates,
  skills, extensions) → messages → editor (border colour encodes thinking level)
  → footer (cwd, session name, token/cache usage, cost, context usage, model).
- **Modes:** ⚠ **there is no plan/normal/edit mode system.** The only modes are
  TUI mode (`regular` vs experimental `fullscreen`), **bash mode** (`!` prefix),
  and **thinking level** (7 levels, `Shift+Tab` cycles).
- **Editor:** multi-line (`Shift+Enter`, `Ctrl+J`), `@` fuzzy file search, Tab path
  completion, kill-ring + undo, jump-to-character, **bracketed-paste collapse**
  (>10 lines → `[paste #1 +50 lines]`), external editor `Ctrl+G`.
- **Message queue (distinctive):** `Enter` queues a **steering** message (delivered
  after the current turn's tool calls); `Alt+Enter` queues a **follow-up** (after
  all work); `Escape` aborts and restores; `Alt+Up` retrieves. Config:
  `steeringMode` / `followUpMode` = `"all"` | `"one-at-a-time"`.
- **Slash commands:** `/login`, `/logout`, `/llama`, `/model`, `/scoped-models`,
  `/settings`, `/resume`, `/new`, `/name`, `/session`, `/tree`, `/trust`, `/fork`,
  `/clone`, `/compact [prompt]`, `/copy`, `/export [file]`, `/import <file>`,
  `/share`, `/reload`, `/hotkeys`, `/changelog`, `/quit`, plus `/skill:<name>` and
  `/<template>`.
- **Keybindings:** `~/.pi/agent/keybindings.json` — **76 bindable actions** across
  `tui.editor.*`, `tui.input.*`, `tui.select.*`, `tui.altScreen.*`, `app.*`. User
  config *replaces* defaults per action; `[]` disables. Emacs and Vim preset blocks
  ship in the docs. ⚠ Global only — **no project-level keybindings file** — and
  strictly single `modifier+key`, **no chords**.
- **Images:** paste (`Ctrl+V`, `Alt+V` on Windows), drag-and-drop, and `@file`.
  Rendered via the **Kitty graphics protocol** (Kitty/Ghostty/WezTerm) or **iTerm2
  inline images**; text placeholder elsewhere. **No Sixel.** PNG/JPEG/GIF/WebP;
  settings `terminal.showImages`, `terminal.imageWidthCells`, `images.autoResize`
  (2000×2000 cap), `images.blockImages`. Not supported on Termux.
- **Themes:** ⚠ **only two built-ins — `dark` and `light`**, auto-detected from the
  terminal background on first run. Custom themes are **JSON** (schema published),
  **51 required colour tokens** + 4 optional, plus `vars` and an `export` section
  for HTML export styling. Discovery: builtin → `~/.pi/agent/themes/*.json` →
  `.pi/themes/*.json` (post-trust) → packages → `themes` setting → `--theme`. The
  active custom theme file **hot-reloads**.
- **Terminal requirements:** Kitty keyboard protocol (with xterm modifyOtherKeys /
  CSI-u fallback), synchronized output (CSI 2026), bracketed paste, OSC 8/11/52/133.
  Per-terminal setup documented for 10+ terminals; tmux needs `extended-keys on`.

## 5. Tools

- **Four tools enabled by default** — `read`, `write`, `edit`, `bash`
  (`docs/quickstart.md`; `src/core/system-prompt.ts` hardcodes
  `selectedTools || ["read", "bash", "edit", "write"]`).
- **Seven built-in tools total** — the above plus **`grep`**, **`find`**, **`ls`**
  (read-only; enabled via `--tools` or the `defaultTools` setting, new in 0.84.2).
  ⚠ The commonly-repeated "Pi has four tools" is only true of the *default
  selection*.
- **Explicitly absent:** no web-search or web-fetch tool, no separate glob tool
  (`find` covers it), no todo tool, no task/subagent tool, no notebook tool.
- **Extensions may register new tools *and override the built-ins*** (see §7),
  including swapping their backends for remote or sandboxed execution.
- **System prompt:** a single template literal in `src/core/system-prompt.ts`,
  **~1,352 chars (~338 tokens)** measured — or ~330 chars without the paragraph
  that lists Pi's own doc paths. Per-tool one-liners, guideline bullets, context
  files, the skills XML block, and `Current working directory:` are appended.
  ⚠ **No primary source states a token count** — see §19(1).

## 6. Skills

- **Standard:** *"Pi implements the [Agent Skills standard](https://agentskills.io/specification),
  warning about most violations but remaining lenient."* One deliberate divergence:
  `name` need **not** match the parent directory, *"suboptimal for shared skill
  directories used across multiple agent harnesses."*
- **Cross-harness reuse is documented as a feature:** the docs show
  `"skills": ["~/.claude/skills", "~/.codex/skills"]` in settings.
- **Locations:** global `~/.pi/agent/skills/` and `~/.agents/skills/`; project
  (post-trust) `.pi/skills/` and `.agents/skills/` in cwd + ancestors to the git
  root; packages (`skills/` dir or `pi.skills`); the `skills` settings array; and
  `--skill <path>` (repeatable, additive even under `--no-skills`).
- **Discovery:** directories containing `SKILL.md` are found recursively
  everywhere; bare root `.md` files count as skills in `~/.pi/agent/skills/` and
  `.pi/skills/` but are ignored in `~/.agents/skills/` and `.agents/skills/`.
- **Lazy loading / progressive disclosure (confirmed):** startup extracts only
  name + description into an XML block in the system prompt; the agent then uses
  `read` to load the full `SKILL.md` on demand — *"only descriptions are always in
  context, full instructions load on-demand."* The docs add an honest caveat:
  *"models don't always do this; use prompting or `/skill:name` to force it."*
- **Frontmatter:** `name` (req, ≤64 chars, `[a-z0-9-]`), `description` (req,
  ≤1024), `license`, `compatibility` (≤500), `metadata`, `allowed-tools`
  (space-delimited, **experimental**), `disable-model-invocation`. Most violations
  warn but still load; **a missing description is the only hard failure**. Name
  collisions warn, first wins.
- **Invocation:** `/skill:<name> [args]`; args are appended as `User: <args>`.
  Toggle with `enableSkillCommands` (default `true`).

## 7. Extensions (TypeScript)

- **Format:** TypeScript `.ts` modules loaded via **jiti — no compilation step**.
  Default export is a factory receiving `ExtensionAPI` (conventionally `pi`); may
  be async. Allowed imports: `@earendil-works/pi-coding-agent`, `typebox`,
  `pi-ai`, `pi-tui`, and Node built-ins.
- **Discovery:** `~/.pi/agent/extensions/{*.ts,*/index.ts}`,
  `.pi/extensions/{*.ts,*/index.ts}` (post-trust), the `extensions`/`packages`
  settings arrays, and `-e`/`--extension` (CLI-loaded ones are **not**
  hot-reloadable). `/reload` hot-reloads auto-discovered extensions.
- **33 events:** `project_trust` · `resources_discover` · session (`session_start`,
  `session_info_changed`, `session_before_switch`\*, `session_before_fork`\*,
  `session_before_compact`\*, `session_compact`, `session_before_tree`\*,
  `session_tree`, `session_shutdown`) · agent/turn/message/provider
  (`before_agent_start`, `agent_start`, `agent_end`, `agent_settled`, `turn_start`,
  `turn_end`, `message_start`, `message_update`, `message_end`,
  `tool_execution_start/update/end`, `context`, `before_provider_headers`,
  `before_provider_request`, `after_provider_response`) · `model_select`,
  `thinking_level_select` · `tool_call`\*, `tool_result` · `user_bash` · `input`.
  (\* = can cancel or block.)
- **API surface:** `ExtensionAPI` has **26 members** (`registerTool`,
  `registerCommand`, `registerShortcut`, `registerFlag`, `registerProvider`,
  `registerMessageRenderer`, `registerEntryRenderer`,
  `registerMarkdownTransformer`, `setActiveTools`, `setModel`, `exec`, …);
  `ExtensionContext` **24** (`ui`, `sessionManager`, `modelRegistry`, `compact()`,
  `getSystemPrompt()`, `isProjectTrusted()`, …); `ctx.ui` **25** (dialogs
  `select`/`confirm`/`input`/`editor`/`notify` with timeout+signal, chrome
  `setStatus`/`setWidget`/`setFooter`/`setTitle`, editor control, autocomplete
  providers, and `ctx.ui.custom()` for full overlays at 9 anchor positions).
- **What extensions can do:** register LLM-callable tools **including overrides of
  the seven built-ins**; inject `promptSnippet`/`promptGuidelines`; replace the
  whole system prompt or the raw provider payload; mutate or block tool calls;
  patch tool results; rewrite LLM context; mutate HTTP headers; render custom TUI;
  register keybindings, CLI flags, and **model providers with custom OAuth**; swap
  tool backends via pluggable `ReadOperations`/`BashOperations` for remote or
  sandboxed execution; contribute skill/prompt/theme paths; persist state.
- **Lifecycle:** no `activate()`/`dispose()` — cleanup goes in an idempotent
  `session_shutdown` handler. Errors are logged and the agent continues, except
  `tool_call` errors, which **block the tool (fail-safe)**.
- **Deferred tool loading (distinctive):** register many tools, keep few active,
  load on demand — with native provider support (Anthropic `defer_loading` +
  `tool_reference` on Sonnet/Opus/Fable ≥4.5; OpenAI `tool_search_call` on
  gpt-5.4+).
- ⚠ **MCP is mentioned zero times** in the 121 KB extensions doc.

## 8. Pi Packages & registry

- **What a package ships:** extensions, skills, prompt templates, and themes —
  declared via a `pi` key in `package.json` or by convention directories
  (`extensions/`, `skills/`, `prompts/`, `themes/`).
- **Commands:** `pi install <source> [-l]`, `pi remove`/`pi uninstall`, `pi list`,
  `pi update`, `pi config [-l]`. ⚠ Not `pi package add`, not `/install`.
- **Three source types:** `npm:@scope/pkg@1.2.3`, `git:github.com/user/repo@v1`
  (also `git@host:path`, `ssh://`, `https://`), and local paths.
- **Install locations:** user npm → `~/.pi/agent/npm/`; project npm → `.pi/npm/`;
  git → `~/.pi/agent/git/<host>/<path>` or `.pi/git/<host>/<path>`.
- `-l` writes to `.pi/settings.json` so a **team shares** the set; Pi auto-installs
  missing project packages at startup after trust. Try without installing:
  `pi -e npm:@foo/bar`.
- **Filtering:** object form in the `packages` setting with per-type glob arrays;
  `!pattern` excludes, `+path` force-includes, `-path` force-excludes.
  **Deduplication** by npm name / git URL sans ref / resolved path; project entry
  wins unless `autoload: false`.
- **Peer-dep rule:** import `pi-ai`, `pi-agent-core`, `pi-coding-agent`, `pi-tui`,
  `typebox` as `peerDependencies: "*"` — never bundle.
- **Registry:** **`https://pi.dev/packages`** — a gallery of npm packages carrying
  the **`pi-package`** keyword, with optional `pi.video` / `pi.image` preview
  metadata. The gallery showed **"1-50 / 5311"** on 2026-08-15; a raw npm search
  for `keywords:pi-package` returns **7,565** (the filtering rule is undocumented).
- **The ecosystem fills exactly the gaps core leaves open** — the top packages are
  `pi-mcp-adapter` (~354.4K downloads/mo, MCP for Pi), `pi-web-access` (~222K/mo,
  web search + fetch), `@vigolium/piolium` (~231.2K/mo, multi-phase audits with
  sub-agents), `pi-hermes-memory`, `@quintinshaw/pi-dynamic-workflows`.

## 9. Prompt templates / custom commands

- Markdown files; **filename = command name** (`review.md` → `/review`).
- **Locations:** `~/.pi/agent/prompts/*.md`, `.pi/prompts/*.md` (post-trust),
  packages, the `prompts` settings array, `--prompt-template <path>`. Discovery is
  **non-recursive**. Disable with `--no-prompt-templates`.
- **Frontmatter:** `description` (falls back to the first non-empty line),
  `argument-hint` (`<required>` / `[optional]`, shown in autocomplete).
- **Argument substitution:** `$1`, `$2`…, `$@` / `$ARGUMENTS`, `${1:-default}`,
  `${@:-default}`, `${@:N}`, `${@:N:L}`.
- Extensions register commands separately via `pi.registerCommand(name, {…})`;
  duplicates get numeric suffixes (`/review:1`). The repo dogfoods templates in
  `.pi/prompts/`.

## 10. Config system & instruction files

- **Files (JSON):** `~/.pi/agent/settings.json` (global) and `.pi/settings.json`
  (project). **Project overrides global; nested objects deep-merge.** Resource
  paths resolve relative to `~/.pi/agent` and `.pi` respectively. There is no
  managed/enterprise or MDM scope, and no remote config.
- **Config directory `~/.pi/agent/`** (override with `PI_CODING_AGENT_DIR`):
  `settings.json`, `auth.json` (0600), `models.json`, `models-store.json`,
  `trust.json`, `keybindings.json`, `AGENTS.md`, `SYSTEM.md` / `APPEND_SYSTEM.md`,
  and `sessions/ skills/ extensions/ prompts/ themes/ agents/ npm/ git/`.
- **Setting groups:** Model & Thinking · UI · Network (`httpProxy`) · Warnings ·
  Compaction · Branch Summary · Retry (agent- and provider-level) · Message
  Delivery · Terminal & Images · Shell · Tools (`defaultTools`) · Sessions ·
  Model Cycling (`enabledModels`) · Markdown · Resources (`packages`, `extensions`,
  `skills`, `prompts`, `themes`, `enableSkillCommands`).
- **Instruction files:** **`AGENTS.md` and `CLAUDE.md` are both supported**;
  **`PI.md` does not exist** (confirmed negative). Load order: `~/.pi/agent/AGENTS.md`
  → `AGENTS.md`/`CLAUDE.md` walking up from cwd → cwd's own. `AGENTS.override.md`
  **replaces** the file *for that directory only*. Context files load **regardless
  of project trust**; `-nc` disables; `/reload` re-reads.
- **System-prompt files:** `.pi/SYSTEM.md` or `~/.pi/agent/SYSTEM.md` **replace**
  the default prompt; `APPEND_SYSTEM.md` appends. CLI equivalents
  `--system-prompt` / `--append-system-prompt`.
- **Env vars:** `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`,
  `PI_PACKAGE_DIR`, `PI_OFFLINE`, `PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY`,
  `PI_CACHE_RETENTION`, `PI_SHARE_VIEWER_URL`, `PI_HARDWARE_CURSOR`,
  `PI_TUI_ESC_TIMEOUT`, `PI_EXPERIMENTAL`, `PI_TUI_WRITE_LOG`, `VISUAL`/`EDITOR`,
  `HTTP_PROXY`/`HTTPS_PROXY`.
- **Process markers** set for children: `AI_AGENT=pi` and `PI_CODING_AGENT=true`
  (⚠ *not* set when embedded via the SDK). **Injected into the LLM-callable bash
  tool only:** `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`,
  `PI_REASONING_LEVEL`.

## 11. Models & providers

- **40 built-in providers** — two independent counts agree: the `KnownProvider`
  union in `packages/ai/src/types.ts` and `builtinProviders()` in
  `packages/ai/src/providers/all.ts`. ⚠ The `pi.dev` homepage says a conservative
  *"15+ providers, hundreds of models."*
  The IDs: `amazon-bedrock`, `ant-ling`, `anthropic`, `azure-openai-responses`,
  `baseten`, `cerebras`, `cloudflare-ai-gateway`, `cloudflare-workers-ai`,
  `deepseek`, `fireworks`, `github-copilot`, `google`, `google-vertex`, `groq`,
  `huggingface`, `kimi-coding`, `minimax`, `minimax-cn`, `mistral`, `moonshotai`,
  `moonshotai-cn`, `nvidia`, `openai`, `openai-codex`, `opencode`, `opencode-go`,
  `openrouter`, `qwen-token-plan{,-cn,-individual}`, `radius`, `together`,
  `vercel-ai-gateway`, `xai`, `xiaomi`, `xiaomi-token-plan-{ams,cn,sgp}`, `zai`,
  `zai-coding-cn`.
- **10 provider API types** (`openai-completions`, `openai-responses`,
  `azure-openai-responses`, `openai-codex-responses`, `anthropic-messages`,
  `bedrock-converse-stream`, `google-generative-ai`, `google-vertex`,
  `mistral-conversations`, `pi-messages`) plus one image API. **Four are
  user-selectable** in `models.json`.
- **~1,267 chat models + 45 image models** across 39 frozen catalogs. Catalogs are
  generated at **build time** from `https://models.dev/api.json` plus live provider
  endpoints and frozen into the package on every publish. ⚠ **models.dev is a
  build-time input only — zero runtime fetches.** Runtime refresh
  (`models-store.json`) is caller-driven with **no TTL**; `allowModelNetwork`
  defaults to `false`.
- **Local models ✅ first-class:** `~/.pi/agent/models.json` explicitly documents
  **Ollama, LM Studio, vLLM, SGLang** and any OpenAI-compatible server; a minimal
  entry needs only `baseUrl`, `api`, `apiKey` (dummy ok), `models: [{id}]`.
  `compat.supportsDeveloperRole` / `supportsReasoningEffort` handle strict servers.
  **The file reloads every time you open `/model` — no restart.**
- **llama.cpp router mode is first-class:** `/login llama.cpp` and `/llama` to
  load / unload / download models with Hugging Face search and quant selection.
- **Model config:** per-model `id`, `name`, `api`, `reasoning`, `thinkingLevelMap`,
  `input`, `contextWindow`, `maxTokens`, `samplingParams`, `cost` (with request-wide
  pricing `tiers`), `compat`; per-provider `baseUrl`, `headers`, `authHeader`,
  `oauth`, `modelOverrides`. Values support `!command` (shell, resolved per
  request, **no built-in TTL by design**) and `$ENV`/`${ENV}` interpolation.
- **Switching:** `/model` or `Ctrl+L`; `Ctrl+P` / `Shift+Ctrl+P` cycle **scoped
  models** (`--models` / `enabledModels` / `/scoped-models`).
- **Reasoning:** a **7-level portable scale** — `off`, `minimal`, `low`, `medium`,
  `high`, `xhigh`, `max` (`xhigh`/`max` opt-in per model via `thinkingLevelMap`).
  `Shift+Tab` cycles; the editor border colour reflects the level;
  `thinkingBudgets` maps levels to token budgets. **11 distinct wire encodings**
  are normalized behind this single scale.
- ⚠ **No small/fast-model split.** No `smallModel`/`fastModel`/background-model
  setting exists; compaction and branch summaries use the **current** model (an
  extension can override via `session_before_compact`).

## 12. Auth / `/login` (subscription reuse)

- **Subscription providers offered by `/login`:**
  - **ChatGPT Plus/Pro (Codex)** — *"Officially endorsed by OpenAI: Codex for OSS."*
  - **Claude Pro/Max** — with an honest documented caveat: *"Third-party harness
    usage draws from extra usage and is billed per token, not against Claude plan
    limits."* Surfaced at runtime by `warnings.anthropicExtraUsage` (default on).
  - **GitHub Copilot** — github.com or a GHES domain; models may need enabling in
    VS Code first.
  - **xAI (Grok/X subscription)**, **OpenRouter** (a PKCE flow that mints a
    user-controlled API key billed from OpenRouter credits, with a headless/SSH
    paste-the-code fallback), and **Radius** (a dynamic `pi-messages` gateway).
- **7 OAuth modules in source:** `anthropic`, `openai-codex`, `github-copilot`,
  `openrouter`, `xai`, `radius`, `kimi-coding`. (The `pi-ai` README lists only 4 —
  source is authoritative.)
- **Credential storage:** `~/.pi/agent/auth.json`, created **0600**. Tokens
  auto-refresh when expired, serialized under a cross-process lock with a 15 s
  refresh timeout. `/logout` clears. ⚠ The standalone `pi-ai` CLI instead writes
  `./auth.json` in the **current working directory**.
- **API keys:** 36+ documented env vars. The `auth.json` key field supports
  `!command` (shell — e.g. `!op read 'op://vault/item/credential'`, or macOS
  Keychain; cached for the process lifetime), `$ENV`/`${ENV}` interpolation, and
  literals, plus a per-credential `env` object for provider-scoped config.
- **Ambient cloud credentials:** AWS profile / IAM keys / bearer token / ECS task
  roles / IRSA; Google Vertex via `gcloud auth application-default login` or
  `GOOGLE_APPLICATION_CREDENTIALS`.
- **Resolution order:** CLI `--api-key` → `auth.json` → env var → `models.json`
  custom-provider keys. Documented invariant: *"A stored credential owns its
  provider: environment variables are only consulted when nothing is stored, and a
  failed refresh never silently falls back to an env key."*

## 13. Permissions, trust & sandboxing — a documented non-goal

Root `README.md`, verbatim:

> *"Pi does not include a built-in permission system for restricting filesystem,
> process, network, or credential access. By default, it runs with the permissions
> of the user and process that launched it."*

`docs/security.md`, verbatim:

> *"Pi does not include a built-in sandbox… This is intentional… A partial
> in-process sandbox would be easy to misunderstand as a security boundary while
> still depending on the host shell, filesystem, package managers, credentials, and
> extension code. Real isolation needs to come from the operating system or a
> virtualization/container boundary."*
>
> *"Prompt injection from repository files, comments, documentation, context files,
> or build output is expected local-agent risk and cannot be reliably prevented by
> pi."*

So there is **no approval model, no allow/deny rule grammar, no permission config,
and no "YOLO" flag — because there is nothing to bypass**.

- **Project Trust** is an *input-loading* guard, not a sandbox. It triggers when cwd
  contains `.pi/settings.json`, `.pi/{extensions,skills,prompts,themes}`,
  `.pi/SYSTEM.md`/`APPEND_SYSTEM.md`, or `.agents/skills`. Decisions are saved by
  canonical directory in `~/.pi/agent/trust.json`; the closest saved decision wins.
  Global fallback `defaultProjectTrust`: `"ask"` (default) | `"always"` | `"never"`.
  Non-interactive modes **never prompt**; `--approve`/`--no-approve` override per
  run. It gates project settings, `.pi` resources, project package installs, and
  project extensions — it does **not** gate `AGENTS.md`/`CLAUDE.md`.
- **Sandboxing is external**, with three documented patterns: the **Gondolin**
  extension (a local Linux **micro-VM**; `pi` and its auth stay on the host while
  the seven built-in tools and user `!` commands route into the VM, cwd mounted at
  `/workspace` write-through; needs Node ≥23.6.0 + QEMU); **plain Docker** (a full
  Dockerfile is provided); and **NVIDIA OpenShell** (policy-controlled
  filesystem/process/network/credential/inference controls, able to keep raw API
  keys outside the sandbox via an `https://inference.local` gateway).
- **Extension-level gating** is the supported DIY path — `tool_call` →
  `{block: true, reason}` — and examples ship: `permission-gate.ts`,
  `protected-paths.ts`, and a `sandbox/` example using
  `@anthropic-ai/sandbox-runtime`.

## 14. Sessions, branching & compaction

- **Storage:** `~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl`, where
  `<path>` is cwd with `/` → `-`. **JSONL, tree-structured** via `id`/`parentId`.
  Header `version: 3` (v1 linear → v2 tree → v3 rename), auto-migrated on load.
  Entry types include `SessionMessageEntry`, `ModelChangeEntry`,
  `ThinkingLevelChangeEntry`, `CompactionEntry`, `BranchSummaryEntry`,
  `CustomEntry`, `LabelEntry`, `SessionInfoEntry`.
- **Four distinct navigation operations:**

  | | `/tree` | `/fork` | `/clone` | `/resume` |
  |---|---|---|---|---|
  | Output | same file | new file | new file | opens existing |
  | View | full tree | user-message selector | current active branch | session picker |
  | Summary | optional branch summary | none | none | — |

  `/resume` picker: search-as-you-type, `Ctrl+P` path, `Ctrl+S` sort, `Ctrl+N`
  named-only, `Ctrl+R` rename, `Ctrl+D` delete. `/tree`: navigate, page,
  `Ctrl+←/→` fold/jump, `Shift+L` label, `Shift+T` timestamps, `Ctrl+O` cycle
  through **five filter modes**.
- **Compaction (auto + manual):** triggers when
  `contextTokens > contextWindow - reserveTokens` (defaults `reserveTokens: 16384`,
  `keepRecentTokens: 20000`, `enabled: true`). Walks backwards to the keep budget,
  picks a cut point that is **never a tool result**, LLM-summarizes using the
  previous summary as iterative context, and appends a `CompactionEntry`. Handles
  **split turns** (a single turn over budget) with two merged summaries. The
  summary has a **fixed structure** — Goal / Constraints & Preferences / Progress
  (Done, In Progress, Blocked) / Key Decisions / Next Steps / Critical Context —
  plus `<read-files>` and `<modified-files>` blocks with **cumulative file tracking**
  across successive compactions. Tool results are truncated to 2,000 chars during
  serialization. Compaction requests use fresh routing session IDs and disable
  prompt-cache writes. `/compact [instructions]` for manual runs.
- **Branch summarization (distinctive):** navigating away from a branch in `/tree`
  offers to summarize the **abandoned** branch and attaches a `BranchSummaryEntry`
  at the new position.
- **Extensions can fully replace compaction** via `session_before_compact` /
  `session_before_tree`, with `convertToLlm()` and `serializeConversation()`
  exported for DIY summarization on a different model.
- **Prompt caching:** `cacheRetention` = `none` / `short` (default) / `long`;
  Anthropic `cache_control.ttl:"1h"`, OpenAI `prompt_cache_retention:"24h"`,
  Bedrock `CacheTTL.ONE_HOUR`; Anthropic-style `cache_control` markers on
  OpenAI-compatible endpoints via `compat.cacheControlFormat:"anthropic"`;
  session-affinity headers in three formats for cache routing;
  `showCacheMissNotices`. The footer shows live token / cache / cost / context use.
- ⚠ **No "context editing" / tool-result-clearing feature** — compaction is the
  only context-reduction mechanism.
- **Export / import / share:** `/export [file]` → HTML (or JSONL), `/import <file>`
  from JSONL, `/share` → a **private GitHub gist with a shareable HTML link**, and
  `pi --export <in> [out]`. ⚠ Programmatically only `export_html` exists.

## 15. MCP & sub-agents — documented non-goals

`docs/usage.md`, verbatim:

> *"It intentionally does not include built-in MCP, sub-agents, permission popups,
> plan mode, to-dos, or background bash. You can build or install those workflows
> as extensions or packages, or use external tools such as containers and tmux."*

- **MCP:** exactly **one** mention across all 30 doc pages (the sentence above);
  zero in the extensions API doc. `packages/coding-agent/README.md`: *"**No MCP.**
  Build CLI tools with READMEs (see Skills), or build an extension that adds MCP
  support."* The community fills it — `pi-mcp-adapter` is the **single
  most-downloaded package in the gallery** (~354.4K/mo).
- **Sub-agents:** *"**No sub-agents.** There's many ways to do this. Spawn pi
  instances via tmux, or build your own with extensions, or install a package."*
  What ships is an **example extension** at
  `packages/coding-agent/examples/extensions/subagent/`: `pi.registerTool()` +
  `pi.exec()`, so each subagent is **a separate `pi` process** with an isolated
  context window — not an in-process agent runtime. Agents are Markdown + YAML
  frontmatter (`name`, `description`, `tools`, `model`; body = system prompt) in
  `~/.pi/agent/agents/*.md` (always) or `.pi/agents/*.md` (only with
  `agentScope: "both"|"project"` **plus interactive confirmation**, since
  repo-controlled prompts are a risk). Three modes: single, **parallel** (max 8,
  4 concurrent, 50 KB output cap per task), and **chain** (with a `{previous}`
  placeholder, stopping at first failure). Four example agents ship (scout,
  planner, reviewer, worker) plus three workflow prompts; install is a **manual
  symlink**.
- Each exclusion carries a one-line rationale in the README's Philosophy section —
  e.g. *"No built-in to-dos. **They confuse models.**"*

## 16. Headless / SDK / RPC / embedding

- **SDK** — ships inside `@earendil-works/pi-coding-agent`; no separate install.
  `createAgentSession({ sessionManager, modelRuntime })` returns an `AgentSession`
  with `prompt`, `steer`, `followUp`, `subscribe`, `setModel`, `setThinkingLevel`,
  `cycleModel`, `navigateTree`, `compact`, `abort`, `dispose`, …. An
  `AgentSessionRuntime` adds `newSession`, `switchSession`, `fork`, `clone`,
  `importFromJsonl`. Custom tools via `defineTool({… parameters /* typebox */ …})`.
  Also exported: `ModelRuntime`, `ModelRegistry`, `SettingsManager`,
  `SessionManager`, all nine tool factories, `InteractiveMode`, `runPrintMode`,
  `runRpcMode`, `RpcClient`. **13 runnable examples** ship under `examples/sdk/`.
- **JSON event stream (`--mode json`)** — JSONL to stdout. First line is the
  session header; then typed events (`agent_start/end`, `turn_start/end`,
  `message_start/update/end`, `tool_execution_start/update/end`, `queue_update`,
  `compaction_start/end`, `auto_retry_start/end`, …). **`message_update` records
  are delta-only** "to keep stream size linear"; `message_end` carries the
  authoritative message.
- **RPC mode (`--mode rpc`)** — line-delimited JSON over stdin/stdout. ⚠ **Not**
  JSON-RPC 2.0, not WebSocket, not CBOR; the docs warn that Node's `readline` is
  non-compliant. **33 commands** including `prompt`, `steer`, `follow_up`, `abort`,
  `new_session`, `get_state`, `get_messages`, `set_model`, `compact`, `bash`,
  `export_html`, `switch_session`, `fork`, `clone`, `get_tree`, `get_commands`,
  plus an **extension-UI sub-protocol** (`extension_ui_request`/`_response`). A
  typed `RpcClient` is exported; Python and Node subprocess examples are in the doc.
- **Experimental CBOR session server (tk):** `pi-protocol` v1 framing
  (`[4-byte BE length][CBOR item]`, 16 MiB limits, strict RFC 8949 subset);
  `pi-server` exports `PiServer` + `createUnixServer`; `pi-client` exports
  `PiClient` with exclusive/shared session leases. ⚠ The `pi-server` README states
  *"This package does not provide a standalone CLI or coding-agent service"* —
  and there is **no `pi serve` command and no `--server` flag** in `args.ts`.
- ⚠ **No ACP (Agent Client Protocol)** — zero hits across docs, READMEs, and repo
  code search.
- **CI:** non-interactive modes never prompt for trust; `--offline`/`PI_OFFLINE`
  disables all startup network operations; `PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY`.
  Git package installs in CI want `GIT_TERMINAL_PROMPT=0` + `GIT_SSH_COMMAND`.
  ⚠ **No official GitHub Action.**
- **No IDE/editor integrations, no web UI.** VS Code and IntelliJ appear only as
  terminal-compatibility notes. The nearest "web" surfaces are `/share` (gist +
  hosted HTML viewer) and the `pi.dev/packages` gallery.

## 17. Adjacent toolkit surface (tk) — noted, not scored

- **`pi-ai`** — the unified LLM API (40 providers, 10 API types, ~1,267 models, the
  7-level thinking scale over 11 wire encodings, OAuth modules, model catalogs). It
  ships its own `pi-ai` binary and is a credible standalone product. Its competitor
  set is the Vercel AI SDK / LiteLLM / `models.dev`, **not** caliban.
- **`pi-agent-core`** — the agent runtime, tool-calling loop, and state model.
- **`pi-tui`** — the differential-rendering terminal-UI library. Its competitor set
  is `ratatui` / Ink / Bubble Tea; caliban *consumes* `ratatui` (ADR-0012) rather
  than publishing a TUI library, so this is not a parity axis.
- **`pi-telemetry`** — vendor-neutral telemetry contracts.
- **Sibling repos** (linked from the README, not part of the monorepo):
  `earendil-works/pi-chat` (Slack/chat automation), `earendil-works/gondolin`
  (micro-VM sandbox), `badlogic/pi-share-hf` (publish sessions to Hugging Face),
  `badlogic/pi-skills`.

These are inventoried so the matrix can say *why* a row is out of scope. Per the
Antigravity precedent they are **not scored as caliban parity gaps**.

## 18. Adoption & currency markers (verified 2026-08-15)

Read from the **GitHub API** and the **npm registry / downloads API** on
2026-08-15. Where these differ from the figures in #515, the figures below are what
the APIs returned this pass — see §19 for the three that diverge.

| Metric | Verified value | Source |
|---|---|---|
| Stars / forks | **90,882** / **11,271** | `gh api repos/earendil-works/pi` |
| License / language | **MIT** / **TypeScript** | GitHub API |
| Repo created | **2025-08-09T14:03:50Z** | GitHub API |
| Latest release | **v0.84.2**, 2026-08-14T10:14:32Z | releases API |
| Total releases | **255** | releases API |
| Releases in the last 60 days | **24** (v0.79.5 → v0.84.2) — ~1 per 2.5 days | releases API |
| First tagged release | **v0.12.0**, 2025-12-02T11:24:16Z ⚠ | releases API |
| Contributors | **271** ⚠ | `gh api …/contributors --paginate` |
| npm 30-day downloads (`pi-coding-agent`) | **6,040,652** (2026-07-11 → 08-09) ⚠ | `api.npmjs.org` |
| Peak single day | **310,887** on 2026-08-07 | `api.npmjs.org` |
| Legacy `@mariozechner/pi-coding-agent` | still **2,500,329** / 30 d | `api.npmjs.org` |
| Sibling packages / 30 d | `pi-tui` 19.7M · `pi-ai` 8.8M · `pi-agent-core` 7.3M | `api.npmjs.org` |
| Open issues | 138 | GitHub API |
| Package gallery | 5,311 shown at `pi.dev/packages`; 7,565 npm `keywords:pi-package` | both |

**Benchmarks: none.** Zero mentions of SWE-bench, Terminal-bench, or any
leaderboard across all primary sources; the README argues explicitly *against*
"toy benchmarks." **Any benchmark claim about Pi in a third-party post is
unsourced.**

**Contributor policy** (README): *"New issues and PRs from new contributors are
auto-closed by default. Maintainers review auto-closed issues daily."*

---

## Notable / distinctive vs caliban

1. **A stated non-goals list.** One sentence, in the docs and on the homepage:
   *no built-in MCP, sub-agents, permission popups, plan mode, to-dos, or
   background bash*, each with a one-line rationale. No other tracked competitor
   states its exclusions this cleanly — and every one of those exclusions is
   something caliban ships.
2. **No permission system or sandbox at all, on purpose** — with an argued case
   that a partial in-process sandbox is *worse* than none because it reads as a
   security boundary while still depending on the host. This is the exact inverse
   of caliban's posture (ADR-0029/0032/0045/0054).
3. **A 5,300–7,500-package ecosystem where the top packages are precisely the
   omitted features** (MCP adapter, web access, sub-agents). Extensibility is not
   an add-on; it is the product strategy.
4. **TypeScript extensions with 33 events and a ~75-member API**, loaded via jiti
   with no build step, able to **override built-in tools** and swap their backends
   for a micro-VM.
5. **Tree-structured sessions in a single JSONL file**, with `/tree` branching in
   place, labels, timestamps, five filter modes, and **branch summarization** of
   abandoned branches.
6. **Steering vs. follow-up as two distinct message queues** (`Enter` vs
   `Alt+Enter`), each with `all` / `one-at-a-time` delivery.
7. **Subscription reuse across four vendors** — ChatGPT Plus/Pro (OpenAI-endorsed),
   Claude Pro/Max, GitHub Copilot, xAI — plus OpenRouter PKCE and Radius.
8. **40 providers / ~1,267 models with 11 reasoning encodings** normalized behind
   one 7-level thinking scale, and first-class **llama.cpp** router integration.
9. **Deferred tool loading** with native Anthropic `tool_reference` and OpenAI
   `tool_search_call` support.
10. **Supply-chain hardening as a documented product feature** — min-release-age,
    shrinkwrap, lifecycle-script allowlist, `--ignore-scripts` throughout,
    reproducible binaries.
11. **A self-documenting agent** — the system prompt embeds paths to Pi's own
    README, docs, and examples, so *"you can also ask the agent to explain itself."*
12. **Release cadence of ~1 per 2.5 days** across 255 releases.

## Explicit uncertainties to re-verify before the next parity pass

- **(1) The "~200-token system prompt" claim — UNCONFIRMED.** No primary source
  states a token count. The measured default template is **~1,352 chars (~338
  tokens)**, or ~330 chars (~83 tokens) excluding the paragraph listing Pi's own
  doc paths, before per-tool snippets, guidelines, context files, and the skills
  block are appended. **Do not assert "~200 tokens."** Cite the source file.
- **(2) Contributor count — DIVERGES from #515.** `gh api …/contributors
  --paginate` returns **271**, not 30 (top: `badlogic` 3,537 commits, `mitsuhiko`
  521, `christianklotz` 142). The 30 figure may have been a commit-threshold count;
  re-derive before reuse.
- **(3) npm 30-day downloads — DIVERGES from #515.** The window ending 2026-08-09
  returns **6,040,652**; no window tried reproduces 6,356,552 (alternatives:
  6,215,723 / 6,310,818 / 6,174,539). The peak-day figure matches exactly, so the
  package is right — likely a boundary or staleness difference. State the window.
- **(4) First tagged release — CORRECTION.** Five v0.12.x tags landed on 2025-12-02
  within ~64 minutes; the earliest is **v0.12.0 (11:24:16Z)**, not v0.12.2.
- **(5) Exit codes** are recovered from source, not documented; RPC-mode exit codes
  are unknown.
- **(6) Package-gallery filtering** — `pi.dev/packages` shows 5,311 while raw npm
  `keywords:pi-package` returns 7,565. The filter rule is undocumented.
- **(7) Default provider** — `args.ts` help says *"default: google"*, but no doc
  page states a default provider. Low confidence.
- **(8) Model/provider counts are not browsable on github.com** —
  `packages/ai/src/providers/data/*.json` are **gitignored**; the 40-provider and
  ~1,267-model counts come from `types.ts` / `all.ts` and the published npm tarball.
- **(9) Image content-block shape differs between docs** — `sdk.md` uses
  `{type:"image", source:{type:"base64", mediaType, data}}` while `rpc.md` and the
  agent-core README use `{type:"image", data, mimeType}`.
- **(10) Undocumented API surface** — `ctx.ui.setHeader()` and
  `ctx.ui.setHiddenThinkingLabel()` appear in extensions.md example tables but not
  in the API body.
- **(11) The experimental `pi-server`/`pi-client`/`pi-protocol` trio** is absent
  from every coding-agent doc page and from the README package table. Whether it is
  intended to become a driveable session server is unstated — recheck next pass,
  because it would change row **C-6**.

---

## Source pages (fetched 2026-08-15)

Canonical docs at `https://pi.dev/docs/latest/<page>`, mirrored from
`https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/<page>.md`.
All **HTTP 200** this pass. `github.com/badlogic/pi-mono` **301 →**
`github.com/earendil-works/pi`.

| Page | Slug | Notes |
|---|---|---|
| Product / homepage | `pi.dev` | tagline, install matrix, "15+ providers" |
| Docs index | `/docs/latest/index` | positioning, package layout, programmatic-usage nav |
| Quickstart | `/docs/latest/quickstart` | 4 default tools |
| Usage | `/docs/latest/usage` | CLI reference, TUI, slash commands, **non-goals sentence** |
| Skills | `/docs/latest/skills` | Agent Skills standard, lazy loading, frontmatter |
| Extensions | `/docs/latest/extensions` | 33 events, full API, deferred tool loading (121 KB) |
| Packages | `/docs/latest/packages` | `pi install`, npm/git/local sources, filtering |
| Prompt templates | `/docs/latest/prompt-templates` | `$@` substitution, `argument-hint` |
| Settings | `/docs/latest/settings` | all setting groups, `AGENTS.md`/`CLAUDE.md` |
| Environment variables | `/docs/latest/environment-variables` | `PI_*` vars, process markers |
| Providers | `/docs/latest/providers` | `/login` subscription reuse, auth resolution order |
| Models | `/docs/latest/models` | `models.json`, local servers, thinking levels |
| Custom provider | `/docs/latest/custom-provider` | provider extension API |
| llama.cpp | `/docs/latest/llama-cpp` | `/llama` router mode |
| Security | `/docs/latest/security` | **no sandbox, by design**; prompt-injection posture |
| Containerization | `/docs/latest/containerization` | Gondolin micro-VM, Docker, OpenShell |
| Sessions | `/docs/latest/sessions` | `/tree` `/fork` `/clone` `/resume`, share/export |
| Session format | `/docs/latest/session-format` | JSONL v3, entry types |
| Compaction | `/docs/latest/compaction` | thresholds, structured summary, branch summaries |
| Keybindings | `/docs/latest/keybindings` | 76 actions, Emacs/Vim presets |
| Themes | `/docs/latest/themes` | 2 built-ins, 51 tokens, JSON schema |
| Terminal setup | `/docs/latest/terminal-setup` | protocol requirements per terminal |
| SDK | `/docs/latest/sdk` | `createAgentSession`, `defineTool`, 13 examples |
| JSON mode | `/docs/latest/json` | delta-only `message_update` |
| RPC mode | `/docs/latest/rpc` | 33 commands, extension-UI sub-protocol |
| TUI library | `/docs/latest/tui` | **(tk)** component API for extension authors |
| Termux | `/docs/latest/termux` | Android support caveats |
| Package gallery | `pi.dev/packages` | 5,311 shown; `pi-package` keyword |
| Install script | `pi.dev/install.sh` | wraps npm |
| Repo README | `raw.githubusercontent.com/earendil-works/pi/main/README.md` | philosophy, non-goals, supply chain |
| Package manifests | `packages/*/package.json` (raw, 9 files) | npm names, versions, `private` |
| Provider source | `packages/ai/src/{types.ts,providers/all.ts}` | 40 providers, 10 API types |
| System prompt source | `packages/coding-agent/src/core/system-prompt.ts` | default tool list, prompt size |
| CLI source | `packages/coding-agent/src/cli/args.ts` | authoritative flag list |
| Subagent example | `packages/coding-agent/examples/extensions/subagent/` | process-per-subagent |
| GitHub / npm APIs | `api.github.com`, `registry.npmjs.org`, `api.npmjs.org` | §18 metrics |
