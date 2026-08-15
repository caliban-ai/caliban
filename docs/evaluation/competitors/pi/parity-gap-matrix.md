# Caliban ↔ Pi parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and **Pi** (`pi.dev`, `earendil-works/pi`) — a minimal terminal
> coding harness and caliban's closest architectural analogue among tracked
> competitors. Refresh it whenever a major feature lands or Pi ships a new
> capability. Use it — alongside the
> [Claude Code](../claude-code/parity-gap-matrix.md),
> [Codex](../codex/parity-gap-matrix.md),
> [Grok Build](../grok-build/parity-gap-matrix.md),
> [OpenCode](../opencode/parity-gap-matrix.md), and
> [Google Antigravity](../antigravity/parity-gap-matrix.md) matrices — to
> prioritize the next sprint.
>
> **How to use it — read the scope note first.** Pi is a **toolkit**, not purely a
> CLI: its GitHub description is *"AI agent toolkit: unified LLM API, agent loop,
> TUI, coding agent CLI."* Applying the precedent set for
> [Antigravity](../antigravity/parity-gap-matrix.md), the rows split three ways:
> - **Head-to-head** rows compare `packages/coding-agent` (the `pi` binary) with
>   caliban. These are genuine apples-to-apples comparisons — same category, same
>   users, same terminal.
> - Rows tagged **(tk)** describe the broader **toolkit** layer (`pi-ai`,
>   `pi-agent-core`, `pi-tui`, `pi-telemetry`) — independently consumable
>   libraries whose competitor set is the Vercel AI SDK / LiteLLM / `ratatui`,
>   **not** caliban. They are listed in §O for context and **are not scored as
>   parity gaps**.
> - Rows tagged **n/a** are concepts neither project intends, or Pi-side surface
>   with no intended caliban analogue.
>
> **This matrix is unusually two-directional.** Pi publishes an explicit non-goals
> list — *no built-in MCP, sub-agents, permission popups, plan mode, to-dos, or
> background bash* — and caliban ships every one of them. Rows where caliban is
> ahead are marked **➕** so the document does not read as a one-way scorecard.
> Those rows are **not** work items; they are the moat worth defending.
>
> When shipping a feature that closes a head-to-head row, tick it 🔴 → 🟡 or
> 🟡 → ✅ in the same PR.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of Pi's documented surface, captured from
> `pi.dev/docs/latest/*` and the repo. That file is the *source* this matrix is
> derived from; refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **➕** caliban
ahead — Pi documents this as a deliberate non-goal or simply lacks it ·
**(tk)** broader-toolkit surface, not scored · **n/a** = neither project intends
it, or no caliban analogue is wanted. A ✅ means "caliban does the equivalent
thing," not byte-identical.

**Last refreshed:** 2026-08-15 (initial pass; #515). The Pi column derives from
[`capability-inventory.md`](capability-inventory.md) snapshot 2026-08-15, read
directly off `pi.dev/docs/latest/*`, the `earendil-works/pi` repo, and the
GitHub/npm APIs — **primary sources only**. The caliban column was **re-verified
against the code on `main` at v0.8.0 (`81ee0ff`)** this pass, not inherited from
the sibling matrices; every 🔴 and 🟡 cites a file path, ADR, issue, or PR.

> **Caveat:** rows tagged **⚠** depend on a Pi fact still flagged uncertain in the
> inventory (§19 there) or on a caliban detail that could not be settled from the
> source this pass. Three rows below also **correct** claims in the sibling
> matrices — they are called out inline.

---

## A. Install & distribution

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| One-line install (npm / `curl \| sh` / PowerShell / pnpm / bun) | 🔴 | caliban documents only `cargo build --release`; `docs/guide/src/getting-started/installation.md` still says *"There are no pre-built releases yet."* All 8 versions through 0.8.0 *are* live on crates.io, but `Cargo.toml:182-187` calls the crates *"plumbing… explicitly internal/unstable"* and no doc teaches `cargo install caliban` |
| Prebuilt standalone binaries (6 targets, `SHA256SUMS`, reproducible builds) | 🔴 | `gh release view v0.8.0 --json assets` returns **zero assets** on every release; no workflow uploads binaries |
| Published container image | ✅ | `ghcr.io/caliban-ai/caliban`, tags 0.5.0–0.8.0 (`docs/container.md`, `.github/workflows/release-image.yml`, PR #298). Pi ships a Dockerfile for sandboxing but publishes no image. ⚠ **corrects** `../codex/parity-gap-matrix.md:44`, which still says the image is "not yet shipped" |
| Self-update (`pi update --self`) | 🔴 | no `update`/`upgrade` variant in `caliban/src/args.rs`; only `caliban plugin update <name>` |
| Windows support | 🟡 | caliban builds on Windows but `crates/caliban-sandbox` has **no Windows backend** (ADR-0032:35-36), so Bash runs unfenced there. Pi requires a bash shell on Windows |
| Android / Termux | 🔴 | no mobile target; Pi ships a dedicated Termux page |
| Homebrew formula | n/a | neither project ships one |
| Supply-chain hardening as a documented feature | 🟡 ⚠ | caliban pins via `Cargo.lock` and publishes from tagged CI (`scripts/publish.sh`), but documents no min-release-age, lifecycle-script allowlist, shrinkwrap, or reproducible-build posture. ⚠ not fully audited against `main` this pass |

## B. Architecture & scope

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Single-process terminal agent (TUI + headless) | ✅ | caliban's core shape (ADR-0012) |
| Monorepo of separately consumable libraries **(tk)** | (tk) | see §O — adjacent surface, not scored |
| Sibling daemon / background agent fleet | ➕ | `caliband` (ADR-0042/0047/0051/0052). Pi has no daemon — the docs suggest *"spawn pi instances via tmux"* |
| Standalone session server (`pi serve`) | n/a | **neither ships one.** Pi's `pi-server`/`pi-client`/`pi-protocol` are experimental and referenced nowhere in the coding-agent docs (inventory §16, uncertainty 11); caliban's is open epic **#503** with ADR spike **#504**. The "no driveable surface" gap the sibling matrices flag is real but is **not** a Pi gap |
| ACP (Agent Client Protocol) | n/a | neither has ACP — zero hits in Pi's docs and repo; caliban tracks it under #503 |

## C. CLI & headless

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Non-interactive print mode (`-p`/`--print`) | ✅ | `caliban -p`, auto-detected on non-TTY (ADR-0025, `caliban/src/headless/`) |
| Structured JSONL event stream | ✅ | `--output-format json/stream-json` NDJSON frames (ADR-0025, enriched by ADR-0049). Pi's `--mode json` sends **delta-only** `message_update`s; caliban's stream is comparable |
| Piped stdin merged into the prompt | ✅ | headless accepts stdin |
| Resume / continue headlessly | ✅ | `--continue` / `--resume <id>` (`caliban/src/headless/session_loader.rs`) |
| Bidirectional RPC over stdio (33 commands + extension-UI sub-protocol) | 🟡 | `--input-format stream-json` handles only `user` and `control/interrupt` frames (ADR-0025). No command surface for model / session / tree / export control, and no UI request-response channel |
| Turn and spend caps | ➕ | `--max-turns`, `--max-budget-usd`. Pi documents no turn or spend cap |
| Extension-defined CLI flags (`pi.registerFlag`) | 🔴 | unknown `--flags` are a clap parse error; plugin manifests contribute no CLI flags (`crates/caliban-plugins/src/manifest.rs:16-68`) |
| Package/extension management subcommands | 🟡 | `caliban plugin {list,info,install,update,remove,enable,disable}` (`caliban/src/plugin_cli.rs:14-35`); no git-URL source and no `pi config`-style enable/disable TUI |
| Credential inspection (`pi auth check` / `print-api-key`) | 🔴 | no `auth` variant in the `CalibanCommand` enum (`caliban/src/args.rs`); `/status` is a stub (`caliban/src/tui/slash/model.rs:134-156`, issue **#3**) |
| Model listing (`--list-models`) | 🔴 | no `caliban models` subcommand; `/model` with no argument lists only the **current provider's** models (`caliban/src/tui/slash/model.rs:18-89`) |
| Session export from the CLI (`pi --export`) | 🟡 | `/export` is TUI-only (`caliban/src/tui/slash/export.rs:23-74`); no export subcommand |
| Offline mode (`--offline` / `PI_OFFLINE`) | 🟡 | `--no-mcp` and env kill-switches exist per subsystem, but there is no single flag that disables all startup network operations |
| Diagnostics subcommand | ➕ | `caliban doctor` / `/doctor`. Pi has no equivalent |

## D. Config & instruction files

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Global + project settings, nested deep-merge | ✅ | five scopes (managed / user / project / local / CLI) with per-key merge semantics (ADR-0045, `crates/caliban-settings/src/{scope,merge}.rs`) |
| Managed / enterprise scope | ➕ | `/etc/caliban/managed-settings.*` + MDM delivery (ADR-0026/0045). Pi documents only global + project |
| `AGENTS.md` + `CLAUDE.md` ingestion with ancestor walk | ✅ | `ANCESTRY_FILENAMES = [".caliban.md", "CLAUDE.md", "AGENTS.md"]` (`crates/caliban-memory/src/project_walk.rs:42`), ADR-0036. ⚠ **corrects** `../antigravity/parity-gap-matrix.md:86`, which marks `AGENTS.md` ingestion 🟡 "verify" |
| `@`-imports with cycle detection | ➕ | depth cap 5 + cycle detection + external-path approval allowlist (`project_imports.rs`, ADR-0036/0050). Pi has no import mechanism |
| Per-directory override file (`AGENTS.override.md`) | 🔴 | caliban concatenates ancestry files with no per-directory replace semantics (`project_walk.rs`) |
| System-prompt replace / append files (`SYSTEM.md`, `APPEND_SYSTEM.md`) | 🟡 | `/output-style` splices a prefix into the prompt (ADR-0031, `compose.rs:1699,1709`), but there is no full-prompt replacement file and no `--system-prompt`/`--append-system-prompt` flag in `caliban/src/args.rs` |
| Env-var interpolation in config | ✅ | `${VAR}`, `${VAR:-default}`, `${CLAUDE_PROJECT_DIR}` (`crates/caliban-common/src/expand.rs`) |
| Live config reload | ✅ | `SettingsWatcher` (notify, 250 ms debounce) |
| Interactive settings editor (`/settings`, `pi config`) | 🟡 | `/config` renders a **read-only** panel — `caliban/src/tui/overlay.rs:479-560`, with zero key handlers in `tui/input.rs` — against ADR-0026's stated tabbed-editor design. Writes go through `caliban config` / `caliban settings`. Issue **#498** tracks related schema drift |

## E. Auth & subscription reuse — highest-leverage gap

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Interactive `/login` provider login | 🔴 | `/login` is a **stub** returning a static string that names "the Auth spec" — `caliban/src/tui/slash/model.rs:158-177`. Issue **#1** |
| Claude Pro/Max subscription reuse | 🔴 | absent. `docs/superpowers/specs/2026-05-24-headless-mode-design.md:38-40`: *"Caliban is provider-agnostic; there's no Anthropic subscription mode to bill against."* Pi documents the same trade-off honestly (billing draws from extra usage, not plan limits) and ships it anyway |
| ChatGPT Plus/Pro (Codex) reuse | 🔴 | absent — no OpenAI OAuth module. Pi's is *"officially endorsed by OpenAI: Codex for OSS"* |
| GitHub Copilot subscription reuse | 🔴 | absent. Copilot appears in caliban only as an example MCP server URL |
| xAI / OpenRouter / Radius subscription or PKCE login | 🔴 | absent |
| Provider OAuth generally (7 modules in Pi) | 🔴 | caliban's OAuth is **MCP-server-only** — `crates/caliban-mcp-client/src/oauth.rs` (PKCE, RFC 8414/9728/7591, OS keyring, hardened across PRs #300/#304, #313/#315, #430/#443) — and **none of it is wired to provider auth**. The machinery exists; the seam does not |
| API-key env vars | ✅ | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `AZURE_OPENAI_*`, `OLLAMA_BASE_URL` |
| Shell-command credential resolution | ➕ | `apiKeyHelper` (`crates/caliban-settings/src/api_key_helper.rs`): shell-free argv spawn, per-provider or `*` wildcard, 5-min TTL, slow-helper warning, `invalidate()` on 401 with a single retry. Pi's `!command` has **no TTL and no invalidation** — caliban's is the better design |
| `/logout` and `/status` | 🔴 | both stubs — `caliban/src/tui/slash/model.rs:179-198` and `:134-156`. Issues **#1** / **#3** |
| Credential store with 0600 perms and auto-refresh | 🟡 | the MCP OAuth store has keyring + 0600-file fallback, but there is **no provider credential store at all** — provider keys live only in env or the helper's output |
| A unified auth spec | 🔴 | no `docs/adr/*auth*` and no auth spec on disk; "the Auth spec" is referenced four times in code and docs but exists only as issue **#6** |

## F. Models & providers

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Provider breadth (40 built-in) | 🟡 | `--provider` accepts only `anthropic \| openai \| ollama \| google` (`ProviderKind`, `caliban/src/args.rs:19-25`); `router::build_one` (`caliban/src/router.rs:84-172`) errors *"unknown provider"* on anything else |
| Bedrock / Vertex | 🟡 | `crates/caliban-provider-{bedrock,vertex}` are library-complete (ADR-0034) but **`caliban/Cargo.toml:31-35` does not depend on them**, and `rg 'bedrock\|vertex' caliban/src/` returns nothing — unreachable from the CLI. ⚠ **corrects** `../claude-code/parity-gap-matrix.md:174-175` and `../codex/parity-gap-matrix.md:114`, which both mark these ✅ |
| Azure OpenAI | 🟡 | a cargo feature on the OpenAI crate, unwired to the CLI. Azure AI Foundry is issue **#30** |
| Local runners (Ollama / LM Studio / vLLM / SGLang / OpenAI-compatible) | ✅ | Ollama is first-class with **dynamic model discovery** (`/api/tags`, `/api/show`, `/api/ps`, XDG-cached); LM Studio and vLLM via `OPENAI_BASE_URL`, probed at [`probes/2026-05-27-lmstudio-probe-findings.md`](../../probes/2026-05-27-lmstudio-probe-findings.md). Dynamic discovery for those two is issues **#318**/**#317** |
| Declarative custom-model file, hot-reloaded on the model picker | 🟡 | `caliban.toml` declares routes and models with a walk-up discovery and a `caliban router debug` inspector (ADR-0038), but adding a **new provider endpoint** still requires a Rust crate, and the file is not re-read when `/model` opens |
| llama.cpp router integration (`/llama`: download, HF search, quant selection) | 🔴 | no analogue — caliban can talk to an OpenAI-compatible endpoint but cannot manage local model weights |
| Fallback chains / hedging / circuit breakers | ➕ | router v2 (ADR-0038): `fallback.rs`, `hedging.rs` (`race_hedged` + cancellation), `breaker.rs` (Closed→Tripped→HalfOpen) in `crates/caliban-model-router/`. Pi has retry settings but no routing layer |
| Purpose-keyed / fast-model split | ➕ | `RequestPurpose::{MainLoop, Summarization, FastClassifier, SubAgent, Embedding, Other}` + `model_overrides` (ADR-0022). Pi has **no** small/fast-model split — compaction and branch summaries run on the current model |
| Runtime model switching across providers | 🟡 | `/model <id>` swaps in place but **same-provider only** — cross-provider returns `CrossProvider` and needs a restart (issue **#31**). Pi switches freely and cycles scoped models with `Ctrl+P` |
| Portable reasoning-effort scale | 🟡 | `Effort::{Low,Medium,High,Max,Auto}` + `ThinkingSetting` decoupled (`crates/caliban-provider/src/effort.rs`, ADR-0038, PR #100) — but exposed **only** via `/effort` and `/think`: no CLI flag and no settings key for the base agent. Pi's 7-level scale normalizes **11 distinct wire encodings** and is settable from the flag, the model string (`sonnet:high`), and `Shift+Tab` |
| Model catalog with cost metadata | ✅ | vendored `crates/caliban-telemetry/rates.yaml` with `effective_from` dates, `CALIBAN_RATES_YAML` override, unknown-model → $0.00 + debounced warning |

## G. Tools

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| `read` / `write` / `edit` / `bash` | ✅ | plus `MultiEdit` — `crates/caliban-tools-builtin/` |
| `grep` / `find` / `ls` | ✅ | `Grep` + `Glob` with match caps |
| Web search and fetch | ➕ | `WebFetch` + `WebSearch` with three selectable backends (Brave / Tavily / Exa). Pi has neither — `pi-web-access` is a ~222K/mo community package |
| Todos | ➕ | `TodoWrite`. Pi's README: *"No built-in to-dos. **They confuse models.**"* |
| Background bash + output/kill | ➕ | `Bash{background}` / `BashOutput` / `KillShell`. Pi lists background bash as a non-goal |
| Notebook editing | ➕ | `NotebookEdit`. No Pi analogue |
| Configurable default tool selection (`defaultTools`, `-t`, `-xt`, `-nbt`) | 🟡 | per-subagent `tool_allowlist` (ADR-0037) and permission rules gate tools, but there is no global default-tool-selection setting and no `--tools`/`--exclude-tools` flag in `caliban/src/args.rs` |
| Deferred / lazy tool loading | 🟡 | `ToolSearch` + lazy MCP schema loading with sticky activation and an LRU cap (ADR-0046, PR #90) — but **MCP-only** (built-ins are never filtered) and **default-off** (`tools.lazy_mcp`), so a fresh install gets none of it. Issue **#16**. Pi's is native-provider-backed (Anthropic `tool_reference`, OpenAI `tool_search_call`) |
| User-registered tools without recompiling | 🔴 | tools are compiled-in Rust; `plugin.json` has **no `tools` component** (`crates/caliban-plugins/src/manifest.rs:16-68`). MCP is the only path — same posture as Claude Code, but strictly narrower than Pi's `pi.registerTool()` |
| Overriding a **built-in** tool | 🔴 | no override seam at all: MCP servers can add tools but cannot replace `Read`/`Bash`/`Edit`/`Write` |
| Pluggable tool backends (route tools into a VM / remote host) | 🔴 | `BashTool` wraps a local `SandboxedShim` (`crates/caliban-tools-builtin/src/shell/bash.rs:10,26,47`); there is no `ReadOperations`/`BashOperations` seam. Pi's Gondolin extension keeps `pi` + auth on the host while routing all seven built-ins into a micro-VM |
| Image input | 🟡 | **`@path` attachment only** (`caliban/src/tui/attach.rs:151,215-238`). Despite ADR-0039, `caliban-images`' `paste_image_from_clipboard` (`clipboard.rs:38`) and `parse_drag_drop_escape` (`dnd.rs:37`) are **never called from the binary**, and `Read` cannot return an image (`fs/read.rs:98-100` is `read_to_string`). ⚠ **corrects** the ✅ "clipboard, `@path`, DnD" claim in the sibling matrices |
| In-terminal image rendering (Kitty graphics / iTerm2 inline) | 🔴 | no inline image display in the TUI |

## H. Extensibility: skills, extensions & packages — highest-leverage overlap

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Agent Skills standard compliance | ✅ | ADR-0019 — `SKILL.md` + YAML frontmatter, `name`/`description` required, malformed skills skipped with a report (`crates/caliban-skills/src/loader.rs:124-156`, PRs #107/#108) |
| Lazy skill loading / progressive disclosure | ✅ | a single `Skill` tool whose *description* is a name+summary list under an 8 KiB budget, with bodies loaded only on `invoke` (`crates/caliban-skills/src/tool.rs:80-182`), plus a 2 KB `## Skills` prompt block. Architecturally the same design as Pi's |
| Configurable skill-path array / cross-harness reuse | 🔴 | roots are **fixed** to `<ws>/.caliban/skills/`, `$XDG_CONFIG_HOME/caliban/skills/`, and plugin dirs (`crates/caliban-skills/src/loader.rs:11-21`). Pi documents `"skills": ["~/.claude/skills", "~/.codex/skills"]` as a first-class setting — a cheap, high-goodwill win |
| Bundled skill library | 🔴 | exactly **one** skill ships — `auto-memory` (`crates/caliban-skills/src/builtins/auto_memory.md`); `crates/caliban-skills` is 555 LOC total. Pi points at `anthropics/skills` and `badlogic/pi-skills` |
| Install a skill from npm / git / a registry | 🔴 | no `caliban skills install`; third-party skills arrive only by manual copy or bundled inside a plugin |
| Scripted in-process extensions with an event API | 🔴 | caliban's extension seam is **hooks**, not scripts: 18 first-class events with 5 external handler types (`ShellCommandHook`, `HttpHook`, `PromptHook`, `AgentHook`, `McpHook` — `hooks_router.rs`, ADR-0024), which can block or modify tool calls. But there is no in-process API to register tools, renderers, keybindings, commands, or UI, and **9 declared events are reserved and never fired** (ADR-0024:28-31). Pi exposes **33 events** and a ~75-member API |
| Extension-registered LLM-callable tools | 🔴 | see §G |
| Extension-registered UI (widgets, overlays, dialogs, autocomplete) | 🔴 | the TUI is compiled-in (`caliban/src/tui/`); no rendering extension point exists |
| Custom slash commands / prompt templates | 🔴 | all 38 slash commands are compiled-in Rust `impl SlashCommand` (`caliban/src/tui/slash.rs:280-293`). There is **no `.caliban/commands/*.md` loader and no `$ARGUMENTS` substitution** — explicitly out of scope in `docs/superpowers/specs/2026-05-24-slash-command-coverage-design.md:302-303`. Issues **#102**/**#103**. Pi gets `/review` from dropping `review.md` in a directory |
| Themes | 🔴 | zero `theme` hits in `caliban/src/`; the only trace is an opaque `tui` settings value (`crates/caliban-settings/src/settings.rs:358`). Issue **#10**. (Pi ships only 2 built-ins but a full 51-token JSON schema with hot reload) |
| Configurable keybindings | 🔴 | every binding is hardcoded in `caliban/src/tui/events.rs`; no keybindings file. Pi exposes **76 bindable actions** in `keybindings.json` with Emacs/Vim presets |
| Package format bundling skills + hooks + agents + MCP + commands | 🟡 | `plugin.json` **declares** `components: {skills, hooks, agents, output_styles, mcp_servers, commands}` (`crates/caliban-plugins/src/manifest.rs:16-68`), but only **one** aggregation is ever consumed: of `PluginManager`'s five methods, `rg 'hooks_configs\|mcp_servers()\|agent_roots\|output_style_roots\|skill_roots' caliban/src/` returns **`skill_roots` only** (`manager.rs:262` → `main.rs:326`). Hooks, MCP servers, and agents are parsed, namespaced, `${CALIBAN_PLUGIN_ROOT}`-expanded, then **discarded**; `components.commands` has no aggregation function at all (`manifest.rs:66`, deferred to ADR-0040, which shipped without closing the loop). ⚠ **corrects** `../claude-code/parity-gap-matrix.md:82`, which marks this ✅ |
| Output styles from a package | 🟡 | works, but **by accident** — `caliban-output-styles` independently globs `$XDG_DATA_HOME/caliban/plugins/*/output-styles/*.md` (`crates/caliban-output-styles/src/loader.rs:70-71`) while `PluginManager::output_style_roots()` is dead code. `force_for_plugin` is provably inert: `compose.rs` hardcodes `enabled_plugins = &[]` and the `/output-style` overlay renders a literal `[force_for_plugin — inert until ADR 0030]` badge (`caliban/src/tui/slash/existing.rs:331-332`) |
| Install from npm / git / local path | 🟡 | `caliban plugin install` supports a marketplace tarball (sha256-verified, trust record in `$XDG_DATA_HOME/caliban/trust/plugins.json`, hardened HTTP client via PRs #158/#187) and `--dir` sideload, but **no git-URL install** — only doc-comment placeholders in `crates/caliban-plugins/src/discovery.rs:2,11`. No signature verification |
| Project-scoped, team-shared package set with auto-install | 🔴 | plugin enablement is **env-var-only** (`CALIBAN_ENABLED_PLUGINS`); the `plugins` settings key is an opaque `serde_json::Value` that is never deserialized (`crates/caliban-settings/src/settings.rs:343-345`), so a repo **cannot commit its plugin set**. Pi's `pi install -l` writes `.pi/settings.json` and auto-installs on next start |
| Public package registry / gallery | 🟡 | one marketplace index URL (`crates/caliban-plugins/src/marketplace.rs`), no published gallery. Pi's `pi.dev/packages` lists 5,311 packages carrying the `pi-package` keyword |
| Try an extension without installing (`pi -e npm:@foo/bar`) | 🔴 | no ephemeral plugin-load flag in `caliban/src/args.rs` |

## I. Permissions, trust & sandboxing — caliban's moat

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Permission rule grammar (allow / ask / deny) | ➕ | ordered rules with globset patterns, dotted-key matching, and workspace normalization (`crates/caliban-agent-core/src/permissions_matcher.rs:63-79`, ADR-0029/0045). **Pi has none by design:** *"Pi does not include a built-in permission system"* |
| Permission modes + auto-classifier | ➕ | six modes (`permission_mode.rs:17-35`) plus a static-then-model auto-mode classifier with an LRU cache (`auto_mode.rs`). Pi lists permission popups as a non-goal |
| OS-level sandbox | ➕ | macOS Seatbelt + Linux/WSL bubblewrap (`crates/caliban-sandbox`, ADR-0032/0054), wrapped around `BashTool`. Pi argues a partial in-process sandbox is *worse* than none and defers to Gondolin / Docker / OpenShell — an externalized answer to the same problem |
| Egress control | ➕ | egress closed by default under `--workspace`, loopback up, `--sandbox-network=allow` escape hatch, secret-named env scrubbing (ADR-0054, PRs #480/#482). Per-hostname allowlists need a proxy — issue **#477** |
| Permission CLI + decision audit log | ➕ | `caliban perms {list,test,explain,add,remove,import,export,audit,lint}` (`caliban/src/perms_cli.rs`) with a JSONL decision log |
| Project-trust prompt before loading repo-supplied config/extensions | 🟡 | caliban loads `.caliban/settings.toml`, `.caliban/skills/`, and project hooks **with no trust gate**. The nearest analogues are the external-import approval allowlist (`<state>/caliban/imports-allowlist.json`, ADR-0036/0050) and `allowed_http_hook_urls` — both narrower than Pi's per-directory `trust.json` with an `ask`/`always`/`never` default |
| Documented prompt-injection threat model | 🟡 | the sandbox ADRs cover confinement, and ADR-0054:120-130 is honest about macOS loopback, but no user-facing security page states the prompt-injection posture as plainly as Pi's `security.md` |

> **Aside (not a Pi-parity item, but found while verifying this section):** caliban's
> own denial message tells users to *"re-run with `--allow 'Bash(<glob>)'"*
> (`permissions.rs:371-374`, echoed at `README.md:402`), but `split_pattern`
> (`permissions_matcher.rs:29-33`) splits on `:` only — so a parens-form rule
> silently never matches. The working form is `Bash:git *`, used correctly at
> `README.md:341,345` and in `docs/examples/permissions.example.toml:29-38`. Worth
> its own bug ticket.

## J. Agents / sub-agents

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| In-process sub-agent primitive | ➕ | `AgentTool` with `tool_allowlist`, `model`, `isolation`, `background`, `maxTurns` (ADR-0021/0037). Pi ships **no** sub-agents; its *example extension* spawns a **separate `pi` process** per subagent |
| Markdown agent definitions with frontmatter | 🔴 | Pi's example reads `~/.pi/agent/agents/*.md` (`name`, `description`, `tools`, `model`, body = system prompt). caliban has **no loader**: `SpawnSpec.frontmatter_path` (`crates/caliban-supervisor/src/proto.rs:95`) is a dead field, set to `None` at every call site (`agents_cli.rs:323,477`, `compose.rs:954`, `tui/events.rs:1005-1009`, `proc.rs:299`, `worker.rs:1075`) and never read. `/agents` is a stub (`dx.rs`). This is also a gap versus Claude Code, Grok Build, and OpenCode |
| Parallel fan-out with a concurrency cap | 🟡 | in-turn tool dispatch is bounded by `parallel_tool_limit` (`agent.rs:25`, ADR-0016), but there is **no cap on concurrently-running background agents** — no `max_agents` anywhere in `crates/caliban-supervisor`. Pi's example caps at 8 tasks / 4 concurrent with a 50 KB per-task output cap |
| Chained agents with output threading | 🔴 | no chain primitive; Pi's example supports `{chain:[…]}` with a `{previous}` placeholder that stops at first failure |
| Per-agent worktree isolation | 🟡 | a real libgit2 implementation exists (`crates/caliban-worktrees/src/manager.rs:148-296`) but is consumed **only on the background/daemon path** (`caliban-supervisor/src/server.rs`). `isolation: "worktree"` **without** `background: true` is a silent no-op — `compose.rs:884-927` never reads `input.isolation`. Pi has no worktree isolation at all |
| Recursion control | ➕ | prevented architecturally — `compose.rs:864-880` snapshots the tool registry *before* registering `AgentTool`, so subagents structurally lack it (ADR-0021) |
| Interactive background agent fleet | ➕ | `caliband` + bidirectional `caliban agents attach` (ADR-0047, `agents_cli.rs:400-452`). 🟡 caveat: the `Ctrl+B` handoff is a placeholder that sends `initial_prompt: "(snapshot)"` rather than real session bytes (`tui/events.rs:1000-1017`) |

## K. Sessions, branching & context

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Persistent session store | ✅ | `crates/caliban-sessions` — atomic JSON under XDG with an async debounce-writer (PRs #471/#506) |
| Tree-structured session with in-place branching (`/tree`) | 🔴 | caliban sessions are **linear**: no `/tree`, no branch navigation, no labels, no filter modes. Pi stores an `id`/`parentId` tree in one JSONL file with five filter modes |
| Fork / clone to a new session | 🔴 | `/fork` and `/clone` have zero hits repo-wide; forking is a stated ADR-0028 non-goal |
| Branch summarization of an abandoned branch | 🔴 | no analogue — Pi summarizes the branch you navigate away from and attaches it at the new position |
| Session picker with rename / delete | 🟡 | `/resume` **lists** sessions with a substring filter but does not swap in place — `caliban/src/tui/slash/session.rs:117-193` says *"full overlay picker (in-place swap) lands in a follow-up"*, so you must exit and re-run `caliban --session <name>`. No rename or delete affordance |
| Auto-compaction | ✅ | on by default at a 0.75 threshold with 2-strike failure backoff (`agent.rs:75-76,153`; `stream/recovery.rs:33-37`). ⚠ this **reverses** ADR-0009's "opt-in compaction, `NoopCompactor` default" and no ADR records the reversal — only `CHANGELOG.md:338-339` (#292/#294) |
| Structured summary schema + cumulative file tracking | 🟡 | caliban compacts but emits no fixed Goal / Constraints / Progress / Decisions / Next-Steps schema and no `<read-files>` / `<modified-files>` tracking carried across successive compactions |
| Split-turn compaction (a single turn over budget) | 🟡 ⚠ | caliban's compactor + `MicroCompact` reduce turn size, but no code path generates the two-summary merge Pi documents for an over-budget single turn. ⚠ not exhaustively traced this pass |
| Extension-replaceable compaction | 🔴 | compaction is compiled-in; there is no `session_before_compact` equivalent and no exported `convertToLlm()`/`serializeConversation()` seam |
| Tool-output supersession janitor | ➕ | `MicroCompact` (`compact.rs:141-241`) — an LLM-free pass that supersedes stale `Read`/`Grep`/`Glob`/`WebFetch` results by key while never superseding `Bash`. No Pi analogue |
| Checkpoint / rewind of **files** as well as conversation | ➕ | content-addressed sha256 checkpoints with verify-all-then-apply and five restore modes (ADR-0028, `crates/caliban-checkpoint/src/restore.rs`). Pi's tree navigation rewinds conversation only, never the working tree. 🟡 caveat: `message_id()` always returns `None` (`restore.rs:244-253`), so granularity falls back to user-message ordinals |
| Prompt caching | 🟡 | **Anthropic only** — `crates/caliban-provider-anthropic/src/ir_convert.rs` maps `CacheControl::Ephemeral`; Google, OpenAI, and Ollama hardcode `cache_control: None` at every conversion site, silently dropping it. No cache-retention tier either (Pi: `none`/`short`/`long` with 1 h / 24 h TTLs and session-affinity headers). Issue **#493** |
| Live token / cache / cost / context display | ✅ | statusline + `/usage`, `/cost`, `/context` (ADR-0033) with `rust_decimal` math |
| Export | 🟡 | `/export` writes Markdown or `--format json` (`caliban/src/tui/slash/export.rs:23-74`), but **HTML export is absent** and the clipboard target is an explicit stub (`export.rs:50-55`) |
| Import a session from a file | 🔴 | no `/import`; Pi round-trips JSONL |
| Hosted share link | 🔴 | no `/share` plane — Pi posts a **private GitHub gist** with a hosted HTML viewer. Local-first by choice, but the gist model needs no first-party infrastructure |
| Steering vs. follow-up message queues | 🔴 | caliban accepts input during a turn but has no two-queue model (`Enter` = steer after the current tool calls, `Alt+Enter` = follow up after all work) with `all`/`one-at-a-time` delivery and `Alt+Up` retrieval |

## L. MCP

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| MCP client | ➕ | full client — stdio + HTTP + SSE, OAuth, elicitation with a bounded queue, resources with URI templates, per-server permission scoping (ADR-0023/0044, `crates/caliban-mcp-client/`). **Pi has no MCP at all**, by design; `pi-mcp-adapter` is the single most-downloaded package in its gallery (~354.4K/mo) |
| MCP management CLI | 🔴 | ⚠ **correction to the sibling matrices:** there is **no `caliban mcp` subcommand** — the `CalibanCommand` enum in `caliban/src/args.rs` has no `Mcp` variant. Management is declarative TOML + the `/mcp` overlay + `--no-mcp`. Both `../opencode/parity-gap-matrix.md:65,138` and `../grok-build/parity-gap-matrix.md:66,150` mark `caliban mcp` ✅ in error |
| MCP sampling / prompts | n/a | not implemented in caliban (zero hits); Pi has no MCP surface to compare against |
| MCP server mode | n/a | caliban exposes none (epic **#503**); Pi has none either |

## M. TUI ergonomics

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Multi-line editor (Shift+Enter / Ctrl+J) | ✅ | kitty protocol with Alt+Enter fallback and trailing-`\` continuation (PR #101) |
| `@` fuzzy file search + path completion | ✅ | `tui/completer.rs` + `attach.rs`, gitignore-aware |
| External editor (Ctrl+G) | ✅ | alt-screen suspend, `$VISUAL`/`$EDITOR`, argv-split so `EDITOR='code --wait'` works |
| Bracketed paste + large-paste collapse | 🔴 | no `EnableBracketedPaste` and no `Event::Paste` handling anywhere — `TerminalGuard::enter()` (`caliban/src/tui.rs:97-116`) pushes only keyboard-enhancement flags, so pastes arrive as key bursts. Pi collapses >10-line pastes to `[paste #1 +50 lines]` |
| Configurable keybindings | 🔴 | see §H |
| Theme selection | 🔴 | issue **#10** |
| Thinking-level cycling with visual feedback | 🟡 | `/effort` and `/think` change effort, but the input border does not encode the level and `Shift+Tab` cycles **permission modes**, not thinking. Pi's border colour is the level indicator |
| Transcript viewer with search | 🟡 | `Ctrl+O` transcript viewer, `[` scrollback dump, `v` open in `$VISUAL` (`tui/transcript_viewer.rs`) — but **no in-transcript search**; Pi added `Ctrl+Shift+F` fullscreen search in 0.84.2 |
| Diff rendering for successful edits | 🔴 | near-miss diffs render only on **failed** Edit/MultiEdit (`fs/match_old.rs:76`); `rg diff caliban/src/tui/render.rs` returns 0 hits |
| Reverse history search | ➕ | `Ctrl+R` cycling Session → Project → AllProjects (`tui/reverse_history.rs`). No Pi analogue |
| Plan mode | ➕ | `/plan` + `EnterPlanMode`/`ExitPlanMode` + Shift+Tab cycle, gated on `Tool::is_read_only`. Pi lists plan mode as a non-goal |
| Statusline | ➕ | Claude-Code-schema-compatible `settings.statusLine` rendered off-thread (`crates/caliban-settings/src/statusline.rs`). Pi's footer is fixed |
| Permission approval modal | ➕ | 4-button ask modal whose "Always" writes a session-scoped rule (`tui/ask.rs`). Pi has no approval UI to model |
| Mouse selection + OSC-52 copy | ✅ | `tui/mouse_select.rs`, `tui/clipboard.rs` |

## N. Ecosystem & distribution of work

| Capability (Pi) | Caliban | Notes |
|---|---|---|
| Official GitHub Action | n/a | neither ships one — caliban's is a deferred sub-project (`docs/guide/src/automation/ci.md` has hand-written recipes only); Pi's CI story is DIY via `-p` / `--mode json` |
| Web UI | n/a | both are terminal-first |
| IDE / editor integration | n/a | neither ships one |
| Third-party package ecosystem | 🔴 | Pi's gallery lists 5,311 packages and its top entries are precisely its omitted features. caliban's marketplace has no published gallery and, per §H, cannot yet load hooks/MCP/agents/commands from a package anyway |
| Anonymous install/update telemetry | n/a | caliban's telemetry is opt-in OTel export (ADR-0033/0053), not a vendor ping; Pi pings `pi.dev/api/report-install` by default |

## O. Toolkit layer — adjacent surface, not scored **(tk)**

Per the [Antigravity precedent](../antigravity/parity-gap-matrix.md), Pi's broader
toolkit is inventoried for context and **deliberately not scored as parity**. A
missing caliban equivalent here is not a gap; it is a different product category.

| Surface (Pi) | Caliban | Notes |
|---|---|---|
| `pi-ai` — unified LLM API (40 providers, 10 API types, ~1,267 models) **(tk)** | (tk) | caliban's provider crates are workspace-internal by policy — `Cargo.toml:182-187` calls them *"plumbing… explicitly internal/unstable."* `pi-ai` competes with the Vercel AI SDK / LiteLLM / `models.dev`, not with caliban |
| `pi-agent-core` — agent loop / tool calling as a library **(tk)** | (tk) | `caliban-agent-core` is published but internal-by-policy; not a parity axis |
| `pi-tui` — terminal-UI library (19.7M downloads/30 d) **(tk)** | (tk) | caliban **consumes** `ratatui` (ADR-0012) rather than publishing a TUI library |
| `pi-telemetry` — vendor-neutral telemetry contracts **(tk)** | (tk) | `caliban-telemetry` is internal; ADR-0053 covers the emission side (OTel GenAI semconv) |
| `pi-protocol` / `pi-server` / `pi-client` — CBOR remote sessions **(tk)** | (tk) | experimental upstream and absent from the coding-agent docs — see §B and inventory uncertainty (11) |

---

## Pi-distinctive gaps worth a ticket

Capabilities Pi has that caliban lacks and that the sibling matrices do **not**
already track — the highest-signal candidates if we chase Pi parity specifically.

1. **Subscription reuse via `/login`** (§E) — Claude Pro/Max, ChatGPT Plus/Pro
   (OpenAI-endorsed), GitHub Copilot, xAI, and OpenRouter PKCE, on a 0600
   credential store with auto-refresh under a cross-process lock. caliban is
   **API-key-only**: `/login`, `/logout`, `/status`, and `/setup-token` are all
   stubs in `caliban/src/tui/slash/model.rs` and the "Auth spec" exists only as
   issue **#6**. The OAuth machinery is *already built* for MCP
   (`crates/caliban-mcp-client/src/oauth.rs` — PKCE, RFC 8414/9728/7591, keyring
   storage); the missing piece is a provider-auth seam, not a protocol
   implementation. This is the single biggest adoption gap across every tracked
   competitor, and Pi shows it can be done without a first-party billing plane.
2. **Finish the plugin system so packages can actually carry anything but skills**
   (§H) — `plugin.json` advertises six component types; exactly one is consumed.
   `PluginManager::{hooks_configs, mcp_servers, agent_roots, output_style_roots}`
   are **dead code** (`crates/caliban-plugins/src/manager.rs:270-293`), the
   `plugins` settings key is never deserialized
   (`crates/caliban-settings/src/settings.rs:343-345`), and `force_for_plugin`
   renders its own `[inert until ADR 0030]` badge. Pi's whole strategy is that the
   ecosystem supplies what core omits — that only works if packages can bundle
   more than one thing and a repo can commit its set. ADR-0030 is accepted but not
   landed; this is a completion ticket, not a new design.
3. **Markdown-defined, file-loaded customization: agents, commands, keybindings,
   themes** (§H/§J/§M) — four separate 🔴s with one shape. Pi gets `/review` from
   `review.md`, a subagent from `agents/scout.md`, a keybinding from
   `keybindings.json`, and a theme from a JSON file. caliban compiles all four in:
   38 Rust `SlashCommand` impls (`caliban/src/tui/slash.rs:280-293`), a dead
   `SpawnSpec.frontmatter_path` (`crates/caliban-supervisor/src/proto.rs:95`),
   hardcoded bindings in `tui/events.rs`, and no theme code at all (issue **#10**,
   **#102**/**#103**). A single "load customization from files" epic would close
   the largest cluster of gaps in this matrix.
4. **Tree-structured sessions with in-place branching** (§K) — `/tree`, `/fork`,
   `/clone`, labels, five filter modes, and **branch summarization** of the branch
   you navigate away from, all in one JSONL file. caliban's sessions are linear and
   `/resume` does not even swap in place
   (`caliban/src/tui/slash/session.rs:117-193`). This composes well with
   checkpointing (ADR-0028), which already gives us the *file*-side rewind Pi
   lacks — the combination would be genuinely ahead of both.
5. **A tool-backend seam** (§G) — Pi's Gondolin extension keeps `pi` and its
   credentials on the host while routing all seven built-in tools into a micro-VM,
   because tools are pluggable (`ReadOperations` / `BashOperations`). caliban's
   `BashTool` wraps a local `SandboxedShim` with no such seam
   (`crates/caliban-tools-builtin/src/shell/bash.rs`). We have the *better*
   built-in sandbox; Pi has the better *architecture* for remote and VM execution.
6. **Cross-harness skill paths + bracketed paste** (§H/§M) — two cheap wins. A
   configurable skills-path array (`loader.rs:11-21` is hardcoded) would let users
   point at `~/.claude/skills`; bracketed-paste support (`caliban/src/tui.rs:97-116`
   registers none) would fix pastes arriving as key bursts.

**Deliberately not chased:** Pi's absent permission system, sandbox, MCP client,
sub-agent primitive, plan mode, todos, web tools, and background bash are all
caliban **➕** rows. They are the moat — a matrix pass should not read them as
"caliban is overbuilt." Pi's own docs frame each as a considered exclusion, and its
ecosystem promptly re-adds several of them as paid-in-attention community packages.

**Explicitly out of scope for this matrix:** the token-efficiency question. Pi's
headline claim is that it sends substantially less context per turn, and caliban's
base system prompt measures ~845 chars (`caliban/src/system_prompt.rs:11-52`)
against Pi's ~1,352 (inventory §5) — but neither figure is a per-turn measurement,
and **no primary source states Pi's often-quoted "~200 tokens"** (inventory §19(1)).
That is a [`probes/`](../../probes/) question, not a matrix row; #515 defers it to a
separate ticket.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in the same PR, ticking
   🔴 → 🟡 or 🟡 → ✅.
2. When Pi ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch
   `pi.dev/docs/latest/*` and the repo), then propagate any new rows here. Pi
   releases roughly **every 2.5 days**, so re-baseline more often than for the
   sibling competitors — a months-old snapshot is stale.
3. Keep the **(tk)** rows in §O unscored. If Pi ever ships a real `pi serve` on top
   of `pi-server`, promote row **B-4** out of **n/a** and into a scored row.
4. Re-check the **➕** rows too: they are the moat, and a Pi release that adds a
   permission system, MCP, or sub-agents to *core* would be the most significant
   competitive change this matrix could record.
5. Resolve any **⚠** rows against Pi's live docs and caliban `main` when you touch
   them — including the three sibling-matrix corrections noted in §A, §D, §F, §G,
   §H, and §L, which should be propagated to those files on their next refresh.
6. Bump the **Last refreshed** date at the top.
