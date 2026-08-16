# Caliban ↔ OpenAI Codex CLI parity gap matrix

> **What this is:** a living checklist of feature parity between caliban
> (this project) and the **OpenAI Codex CLI**. Refresh it whenever a major
> feature lands or Codex ships a new capability. Use it — alongside the
> [Claude Code matrix](../claude-code/parity-gap-matrix.md) — to prioritize
> the next sprint.
>
> **How to use it:** Codex is a *second* reference agent. Most core surfaces
> caliban already tracks against Claude Code (permissions, hooks, MCP,
> sub-agents, model router, sandbox, headless, OTel); this matrix's job is to
> surface where **Codex differs** — the capabilities Codex has that caliban
> does not, and the places caliban's design diverges. When planning what to
> build, look here for Codex-distinctive gaps; when shipping, tick the row(s)
> in the same PR that ships the code.
>
> **Companion document:** [`capability-inventory.md`](capability-inventory.md)
> — a structured, dated snapshot of Codex's documented surface, captured from
> the canonical docs (`developers.openai.com/codex/*` → `learn.chatgpt.com`).
> That file is the *source* this matrix is derived from; refresh both together.

**Legend:** ✅ caliban has an equivalent · 🟡 partial · 🔴 gap · **n/a** =
Codex-surface concept with no intended caliban analogue (e.g. hosted cloud). A
✅ here means "caliban does the equivalent thing," not that the two are
byte-for-byte identical.

> **Counting convention (shared across the sibling matrices).** Counts are
> **capability-table rows in the lettered sections** — the *Codex-distinctive
> gaps* list and any roadmap/audit tables are excluded. A **down-tick** is a
> row whose rating got worse, *including* a combined row split into
> worse-scoring halves; an **up-tick** is the reverse; deleting a duplicate
> row is neither. A change that rewrites only the Notes cell without moving
> the rating is a **note-only correction** and is counted separately. This
> matrix has **75** scored rows.

**Last refreshed:** 2026-08-16 (**production-call-path re-sweep, #555** — see
the correction block below; the Codex inventory snapshot stays 2026-07-27, #505).
Prior refresh 2026-08-15 (**caliban-side scoring sweep, #519** — no
upstream re-baselining). Every ✅ row was re-verified against `main` at v0.8.0
under the rule now written down in [`../../README.md`](../../README.md#scoring-rule-for-parity-matrices):
a row is ✅ only when a **production call path from the shipped binary**
reaches it. **15 down-ticks, 1 up-tick, 5 note-only corrections** across 21 of
75 rows. The dominant pattern is the one #516 found on the Claude Code matrix —
machinery that compiles and is unit-tested but has no non-test caller
(checkpoint/`/rewind`, image ingest, sandbox deny-maps and domain ACLs, 5 of 6
OTel metrics, 4 of 5 plugin aggregations). Four rows corrected confirmed
defects relayed from PR #517: the GHCR image (§A, shipped — the note said
otherwise), `caliban mcp` (§B, the subcommand does not exist), image ingest
(§C), and the Bedrock/Vertex provider list (§G). The lone up-tick is
`AGENTS.md` (§H), which was **understated** — it is on the live ancestor walk —
and which also retires item 5 of the distinctive-gaps list. Prior refresh
2026-07-27 (primary-source refresh of the Codex facts — derived from
[`capability-inventory.md`](capability-inventory.md) snapshot 2026-07-27;
caliban state cross-referenced from the [Claude Code parity
matrix](../claude-code/parity-gap-matrix.md) as of its 2026-06-17 refresh;
#485). That pass corrected Codex-side facts and notes only — caliban ratings
were not re-verified against `main`.

**Subsequent sweep 2026-08-16 (#555):** **2 down-ticks, 0 up-ticks, 2 note-only
corrections** — a re-application of the production-call-path rule to all 75 rows,
prompted by the sibling sweeps in #550/#551. Every **✅** row was re-traced to a
named production call site this time, not just spot-checked; the row count is
unchanged at **75** (28 ✅ / 26 🟡 / 18 🔴 / 3 n/a). The first down-tick is §C, row
*"`/compact`, `/status` (context + rate limits)"* (✅ → 🟡) — an **internal
contradiction** #519 left behind: §L already scores the same capability 🟡 on the
finding that `/status` is a stub and no rate-limit surface exists. The second is
§J, row *"Worktree isolation"* (✅ → 🟡), a **cross-matrix** reconciliation:
`caliban_worktrees::WorktreeManager` does have a live caller
(`crates/caliban-supervisor/src/server.rs:678-684`), but only on the **background**
path, and the tool schema advertises `isolation: "worktree"` without saying that
`background: true` is also required — so the documented default call is a silent
no-op. The [Antigravity](../antigravity/parity-gap-matrix.md) (#554/#559) and
[Grok Build](../grok-build/parity-gap-matrix.md) (#560) matrices reached 🟡 on this
same code; tracked as **#557**. The note-only
corrections are §A, row *"Windows support"*, where the note overstated confidence
in an untested platform, and §C, row *"Image input"*, whose bare line anchor for
the text-attach skip had drifted (`:146` → `:151`) — the defect class #551 named,
now re-anchored to the guard's symbol. The other 72 rows held: the ✅ rows named in this file's
prior sweeps were each re-confirmed to have a live caller (`merge_with_global` →
`caliban/src/startup/compose.rs:1138`; `caliban_memory::load` →
`compose.rs:1718`; `WebSearchTool` → `compose.rs:611`; `router::try_load` →
`caliban/src/main.rs:281`). The dead capabilities
confirmed on `main` by #550/#551 — `CheckpointHook` (#549), `ElicitationBridge`
and `--permission-prompt-tool`, MCP resources, `SettingsWatcher::watch`, four of
five plugin aggregations, Bedrock/Vertex (#537), `resolve_image_attachments` —
were each re-verified here and **no additional row rests on any of them**: §B
*"`codex fork`"* and §I *"Plugin marketplace"* already carry them, §G's note
already retired the Bedrock/Vertex claim, and this matrix has no live-config-reload
or MCP-resources row at all. §F *"Per-server enable / tool allow-deny / approval
mode"* was checked specifically against the elicitation finding and is **not**
built on it — it rests on `ServerPermissions{allow,deny,ask}` compiled by
`merge_with_global`, which is production.

> **Open convention question, deliberately not settled by this sweep.** §C row
> *"Image input"* is arguably **🔴** under a strict reading of the
> production-call-path rule: no reachable path lets a user supply an image at
> all, so nothing about the capability is user-reachable. It is scored **🟡**
> here only on the convention that ADR-0039's provider-side `ImageBlock` wire
> support earns yellow — the same convention every sibling matrix applies
> ([opencode](../opencode/parity-gap-matrix.md) §H, [grok-build](../grok-build/parity-gap-matrix.md) §*"Image input"*,
> [antigravity](../antigravity/parity-gap-matrix.md) §*"Image input"*, [pi](../pi/parity-gap-matrix.md) §G).
> Changing it is a **cross-matrix convention decision for the repo owner**, not a
> per-file call: it would move five rows across five files at once. Flagged here
> so the inconsistency between the rule as written and the convention as
> practiced is on the record rather than silently resolved in one direction.

**Subsequent correction 2026-08-16 (#524):** **0 down-ticks, 0 up-ticks, 1
note-only correction** — §A, row *"npm (`@openai/codex`) / Homebrew cask / shell
+ PowerShell installers"*. Same counting convention as above; the row count is
unchanged at **75**. This is the mirror image of the #519 failure mode: not a
row scored optimistically, but a shipped capability described as absent. The
note claimed caliban only "builds from source via `cargo`" when the crates.io
channel has shipped on every `v*` tag since 0.1.0. The **rating is deliberately
left 🔴** — npm, the Homebrew cask, and the shell/PowerShell installers all
genuinely do not exist.

> **Caveat:** rows tagged **⚠** depend on a Codex fact still flagged uncertain
> in the inventory (§14 there). The "caliban detail inferred from the Claude
> Code matrix rather than re-verified against `main`" half of this caveat was
> **retired 2026-08-15** — every caliban rating in this file is now verified
> directly against `main`.

---

## A. Install & distribution

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| npm (`@openai/codex`) / Homebrew cask / shell + PowerShell installers | 🔴 | None of the four channels this row names ship, so the rating stands. **Note corrected 2026-08-16 (#524):** the old note said caliban "builds from source via `cargo`; no published npm/brew/installer channel yet", which understated distribution — caliban **is** published to crates.io on every `v*` tag (`.github/workflows/publish.yml`, `scripts/publish.sh`), all 8 versions through 0.8.0 are live and unyanked, and the published `caliban` crate carries `[[bin]] name = "caliban"` (`caliban/Cargo.toml`), so `cargo install caliban` yields a working binary. That is a package-manager channel — just not one Codex offers, so it closes no cell in *this* row. **Note corrected 2026-08-15 (#519):** the parenthetical claim that the container image is "not yet shipped" was stale — `ghcr.io/caliban-ai/caliban` is published multi-arch on every `v*` tag (`.github/workflows/release-image.yml`, `docs/container.md`, PR #298). The image does not close *this* row either, but it is shipped |
| Prebuilt binaries macOS (arm64+x86_64) / Linux (x86_64+arm64) | 🔴 | release-binary distribution not yet stood up |
| Windows support | 🟡 | caliban carries a handful of Windows-conditional arms and runs under WSL; OS sandbox on Windows is deferred (see E — `detect` returns `Backend::Unavailable` and warns, `crates/caliban-sandbox/src/detect.rs:71-81`). **Note corrected 2026-08-16 (#555):** the old note said caliban "runs on Windows/WSL for most paths", which overstated confidence on two counts. First, coverage: repo-wide there are **12** `cfg` lines whose predicate mentions Windows — only **5 positive arms in production code** (`caliban-plugins/src/manager.rs:113`, `manifest.rs:292`, `caliban-settings/src/scope.rs:118`, `caliban-worktrees/src/symlinks.rs:61`, `caliban-sandbox/src/detect.rs:71`), plus 3 inside `#[cfg(test)]` modules and 4 `not(…)` exclusions. (The "~53 `cfg` sites" figure in the #555 issue body is an **overcount** — 53 is every `.rs` line matching *windows*, 41 of which are prose, doc comments and log strings.) Second, verification: `.github/workflows/ci.yml` runs `ubuntu-latest` on all three jobs (`:27`, `:89`, `:154`), so caliban is **never built or tested on Windows or macOS in CI**. The rating stands — "runs on Windows" is untested, not disproven |

## B. CLI subcommands

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Non-interactive run (`codex exec`) | ✅ | caliban `-p`/`--print` headless mode (ADR-0025) |
| `codex resume` (continue a session) | ✅ | session persistence + `/resume`; headless `--resume` |
| `codex fork` (branch a session) | 🔴 | **Down-ticked 2026-08-15 (#519).** The 🟡 was awarded for "`/rewind` + Esc-Esc fork-from-checkpoint partial", but **checkpointing is unreachable in the shipped binary**: `CheckpointHook` has no construction site outside `crates/caliban-checkpoint`'s own tests, and `App::with_checkpoint_store` (`caliban/src/tui/app.rs:573`) is marked `#[allow(dead_code, reason = "wired by main.rs once full /rewind action plumbing lands")]` with zero callers, so `app.checkpoint_store` is always `None` and the overlay renders "(checkpointing not enabled for this session)" (`caliban/src/tui/overlay.rs:826`). Nothing to fork from. Crate is complete and unit-tested (ADR-0028) — machinery, not a shipped path |
| `codex apply` (apply a diff to the tree) | 🔴 | no standalone diff-apply subcommand; caliban's agent edits in place |
| `codex review` (non-interactive review) | 🔴 | `/code-review` is skill-level, deferred to the Skills polish sub-project |
| `codex mcp` (manage MCP servers) | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** There is **no `caliban mcp` subcommand** — `CalibanCommand` (`caliban/src/args.rs`) has no `Mcp` variant (verbs are Router/Agents/Daemon/Attach/Logs/Stop/Kill/Respawn/Rm/Doctor/Config/Plugin/Perms/Settings). Management is declarative TOML (`mcp.toml`) + `--no-mcp` + the `/mcp` overlay, and the overlay's per-server action keys are toast stubs ("disable not yet wired", `caliban/src/tui/events.rs:1139-1173`). Real: connection, per-server `disabled`, and OAuth (ADR-0023). Pi's narrower *"MCP management CLI"* row scores the same evidence 🔴 |
| `codex mcp-server` (Codex **as** an MCP server) | 🔴 | caliban is an MCP client only; exposing caliban itself as an MCP server is unbuilt |
| `codex app-server` (programmatic local server) | 🔴 | no analogue; caliban has no programmatic local server surface to drive it (sibling to `mcp-server`; no ACP surface documented for Codex either) |
| `codex plugin` + `marketplace` | ✅ | `caliban plugin {install,list,enable,disable,remove,info,update}` + marketplace (ADR-0030) |
| `codex sandbox` (run a command under a policy) | 🟡 | caliban has OS sandbox but no standalone "sandbox an arbitrary command" subcommand (ADR-0032) |
| `codex execpolicy` (evaluate rule files) | 🟡 | `caliban perms lint/test/explain` cover rule evaluation; not a 1:1 exec-policy evaluator |
| `codex doctor` | ✅ | `caliban doctor` headless + `/doctor` |
| `codex cloud` / `cloud-tasks` | n/a | no hosted cloud plane (see M) |

## C. Interactive TUI

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| `/` command menu + `@` file mention/search | ✅ | slash-menu typeahead (#15) + `@file` autocomplete (ADR-0027) |
| `/permissions` presets (Auto / Read Only / Full Access) | ✅ | permission modes + Shift+Tab cycle + status chip (ADR-0029) |
| `/reasoning` (adjust effort) | ✅ | `/effort` + `/think` runtime controls (ADR-0038, #100) |
| `/compact`, `/status` (context + rate limits) | 🟡 | **Down-ticked 2026-08-16 (#555).** The context half is real and production: `/compact`, `/context` and `/usage` are registered by `observe::register` (`caliban/src/tui/slash/observe.rs:409-411`), reached from `caliban/src/tui/slash.rs:284` (ADR-0033). The **rate-limit half does not exist**: `/status` is a stub that prints the provider name plus "(full provider/auth/subscription status arrives with the Auth spec)" (`caliban/src/tui/slash/model.rs:148-155`), and no adapter reads `anthropic-ratelimit-*` / `x-ratelimit-*` response headers — the sole `rate_limit` occurrence outside `caliban-model-router`'s tests is an error-category bucket on the headless event stream (`caliban/src/headless/events.rs:132`). The ✅ was an **internal contradiction**: §L row *"In-session context + rate-limit status (`/status`)"* scores the same capability and was already down-ticked to 🟡 for exactly this reason by #519, which did not reach this row |
| `/review` (code-review mode) | 🔴 | skill-level, deferred |
| Raw-output copy (`Ctrl+O` / `Alt+R`, `tui.raw_output_mode`) | 🟡 | `Ctrl+O` transcript viewer + `[` dump-to-scrollback exists; single-response raw-copy chord not a direct match |
| Image input (`--image` / paste) | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** The claimed "clipboard, `@path`, DnD" ingest is **all dead code**. `resolve_image_attachments` (`caliban/src/tui/attach.rs:218`) is marked `#[allow(dead_code, reason = "wired into a follow-up TUI input slice")]` and its only callers are its own unit tests (`attach.rs:480,499`); `caliban-images`' `paste_image_from_clipboard` (`clipboard.rs`) and `parse_drag_drop_escape` (`dnd.rs`) have no callers outside their own modules; the text attach path *skips* image files (the `path_is_image_like` guard inside `resolve_attachments`, `attach.rs:151` — **line anchor corrected 2026-08-16 (#555)**, the note said `:146`); `Read` is text-only (`crates/caliban-tools-builtin/src/fs/read.rs`); there is no `--image` flag. 🟡 reflects the ADR-0039 provider-side `ImageBlock` wire support, which is real — **not** a user-reachable ingest path. ⚠ This also **corrects Pi's §F characterization** ("`@path` attachment only"): `@path` image attachment does not work either |

## D. Config system

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| TOML config (`~/.codex/config.toml` + project `.codex/`) | ✅ | `caliban.toml` + layered settings (managed/user/project/local); TOML primary (ADR-0026/0045) |
| Named profiles (`--profile`) | 🟡 | model-router routes/effort-maps cover some of this; no first-class named-profile switch |
| Enterprise policy layer (`requirements.toml`, managed-hooks gate) | 🟡 | **Down-ticked 2026-08-15 (#519).** The managed settings scope and `permissions.enforce` lockdown are real (ADR-0026/0045) — but the **managed-hooks gate is not**: `allow_managed_hooks_only` fires no hooks at all until hook scope provenance lands (#124), so it is a kill switch, not a policy layer (see the Claude Code matrix §B, row *"Config hooks (`[[hooks.*]]`) execute at runtime"*) |
| Inline overrides (`-c KEY=VALUE`) | 🟡 | `--settings` (file/inline JSON) + `--setting-sources`; per-key `-c` dotted override not a direct match |
| Published schema for editor autocomplete | 🟡 | **Down-ticked 2026-08-15 (#519).** The Draft-7 schema exists and is used — but only *internally*, for load-time validation (`SCHEMA_JSON` / `validate_value`, `crates/caliban-settings/src/{lib.rs:65,schema.json}`). It is **not published** for editors: no hosted URL, no SchemaStore entry, and no dump verb (`ConfigCommand` is `Print`/`Migrate` only, `caliban/src/args.rs:922`). Known drift: the schema rejects a valid key and accepts a phantom one (#498) |

## E. Approval modes & sandboxing

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Orthogonal sandbox × approval axes | ✅ | permission rules/modes (approval axis) + OS sandbox (boundary axis) compose independently (ADR-0029/0032/0045) |
| Sandbox modes (`read-only` / `workspace-write` / `danger-full-access`) | 🟡 | **Down-ticked 2026-08-15 (#519).** `workspace-write` and `danger-full-access` have equivalents (`--workspace` write fence, ADR-0048; no fence by default). There is **no `read-only` equivalent and no user-facing allow/deny map**: the runtime policy is hardcoded in `caliban/src/startup/compose.rs::workspace_fence_policy` (`allow_read: ["/"]`, `compose.rs:465-494`), and `deny_read`/`deny_write` (`crates/caliban-sandbox/src/config.rs:24,31`) are rendered by both backends but populated by **no production call site** — `Policy::from_toml_str` has zero callers. `SandboxSettings` exposes exactly one key, `network` (`crates/caliban-settings/src/settings.rs:65-71`). It is a write fence, not a mode spectrum |
| Approval policies (`untrusted`/`on-request`/`never`) | 🟡 | caliban modes (`default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/`bypassPermissions`) overlap but don't map 1:1. (`on-failure` is deprecated upstream — not a real gap; 🟡 reflects the mode-shape divergence, not a missing policy) |
| macOS Seatbelt enforcement | ✅ | ADR-0032 |
| Linux `bubblewrap` / user-namespace enforcement | ✅ | ADR-0032 (Linux/WSL) |
| Windows native sandbox | 🔴 | Windows sandbox deferred |
| Network-access gating in workspace-write | 🟡 | **Down-ticked 2026-08-15 (#519).** Coarse egress gating is real and default-deny under `--workspace` (ADR-0054, `caliban/src/startup/compose.rs:512-515`; opt out via `--sandbox-network=allow` / `sandbox.network`). But `allow/denyDomains` and the proxy knobs **do not exist as a user surface** — `allowed_domains`/`denied_domains`/`http_proxy_port`/`socks_proxy_port` (`crates/caliban-sandbox/src/config.rs:105-118`) are set by nothing in `caliban/src` or `caliban-settings`, `validate_policy` rejects a domain list without a proxy port (`crates/caliban-sandbox/src/shim.rs`), no loopback proxy ships, and neither backend can filter by name (`seatbelt.rs:133`, `bwrap.rs`). It is all-or-nothing. Tracked in **#477 (open issue, not a landed PR)** |

## F. MCP

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| MCP client, stdio + streamable-HTTP transports | ✅ | rmcp client; stdio + HTTP/SSE (ADR-0023) |
| Per-server enable / tool allow-deny / approval mode | ✅ | per-server permission scoping + `enabled_tools` equivalents |
| OAuth / bearer auth for HTTP servers | ✅ | PKCE + loopback OAuth, keyring store (ADR-0023 Phase C) |
| Startup / tool timeouts | ✅ | `CALIBAN_MCP_TIMEOUT` / `CALIBAN_MCP_TOOL_TIMEOUT` |
| Codex **as** MCP server (`mcp-server`) | 🔴 | see B — unbuilt (Codex `mcp-server` existence confirmed; caliban gap stands) |

## G. Models & providers

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Multiple providers + local models (`--oss`, ollama/lmstudio) | ✅ | **Note corrected 2026-08-15 (#519) — the provider list was wrong, the rating stands.** The binary can construct **four** providers: Anthropic / OpenAI / Ollama / Google (`ProviderKind`, `caliban/src/args.rs:88-95`; `build_provider`, `caliban/src/startup/compose.rs:161-180`; router arms `caliban/src/router.rs:90-150`). **Bedrock and Vertex are not among them**: `caliban/Cargo.toml` does not depend on `caliban-provider-bedrock` or `caliban-provider-vertex`, so no CLI path can construct either — the crates are library-complete per ADR-0034 but reachable only from their own integration tests. The "(broader set)" claim is dropped. Multiple providers + local models is still genuinely ✅: ollama + LM Studio probed |
| Reasoning-effort tiers | ✅ | `low`/`medium`/`high` + effort map (ADR-0038); `/effort` runtime |
| `ultra` tier that auto-delegates to subagents | 🔴 | no effort tier that automatically fans out to a subagent fleet |
| Live web search (`--search`) | ✅ | `WebSearch` (Brave/Tavily/Exa) |
| Provider wire-API selection (Responses vs Chat) | 🟡 | caliban targets Anthropic + OpenAI wire shapes; no Responses-vs-Chat toggle abstraction |

## H. Memory / project instructions

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Nested instruction file, closer-dir-wins precedence | ✅ | CLAUDE.md ancestor walk + nested-on-demand (ADR-0036) |
| `AGENTS.md` as the primary instruction file | ✅ | **Up-ticked 2026-08-15 (#519) — the old note was factually wrong.** `AGENTS.md` *is* read as a live instruction source, on the same ancestor walk as CLAUDE.md: `ANCESTRY_FILENAMES = [".caliban.md", "CLAUDE.md", "AGENTS.md"]` (`crates/caliban-memory/src/project_walk.rs:42`, ADR-0036), with closer-dir-wins precedence. `/init` ingestion is a *separate*, additional path (`caliban/src/tui/slash/session.rs:76-114`). The only residual divergence is naming precedence within a directory — cosmetic, not a capability gap |
| Model-written per-project memory | ✅ | auto-memory (ADR-0035) |
| Cross-session "Memories" / "Chronicle" | 🔴 ⚠ | partly a Codex app/cloud feature; no caliban equivalent to cross-session learned memory |

## I. Hooks / skills / plugins / notifications

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Lifecycle hooks (`PreToolUse`/`PostToolUse`/`SessionStart`/`SessionStop`) | 🟡 | **Down-ticked 2026-08-15 (#519).** The *in-process* taxonomy is a superset and really dispatches (ADR-0024). But Codex's hooks are **config-file** hooks, and a caliban `[[hooks.*]]` handler can only bind to three events: `event_supported` admits `PreToolUse` / `PostToolUse` / `SessionStart` only (`crates/caliban-agent-core/src/hooks_router.rs:250-252`) — a `[[hooks.SessionEnd]]` handler is built and then warn-skipped (`:289-298`). Handler kinds are also partial: only `command` and `http` execute; `mcp`/`prompt`/`agent` are v1 stubs that log and return `Allow` (`hooks_router.rs:344-350`) |
| Regex `matcher` on hooks | 🟡 | **Down-ticked 2026-08-15 (#519).** Matcher filtering is real and production (`hooks_router.rs:152-161`, defaults to `"*"`), but it is **glob, not regex** — `matches_glob` → `globset` (`crates/caliban-agent-core/src/permissions_matcher.rs:42-51`); no `regex` crate participates in hook matching. Alternation `{a,b}` and `*` work; anchors, groups, and character classes beyond globset's do not |
| Enterprise `allow_managed_hooks_only` | 🔴 | **Down-ticked 2026-08-15 (#519).** The key parses and layers (`crates/caliban-agent-core/src/hooks_config.rs:126,199`; `crates/caliban-settings/src/settings.rs:315`; schema entry), but the router does not filter by scope — it **disables every config hook and warns**: "handler scope is not tracked; firing no config hooks (see #124)" (`hooks_router.rs:274-280`), and `docs/guide/src/configuration/reference.md:54` documents it as such. Setting it is a kill switch, not an enterprise gate. (Managed-scope *settings* layering is separately real — `crates/caliban-settings/src/loader.rs:295-298` — and `permissions.enforce` is production, `caliban/src/main.rs:299`) |
| Skills (`SKILL.md`, `.agents/skills`, open standard) | 🟡 | caliban ships skills, but under `.caliban/`/`.claude/` layout, not the `.agents/skills` open-standard path |
| Plugin marketplace (skills + MCP + hooks + connectors) | 🟡 | **Down-ticked 2026-08-15 (#519), correcting a confirmed defect.** The marketplace and the seven CLI verbs are real (`crates/caliban-plugins/src/{cli,marketplace}.rs`, HTTP index + `.tar.gz` + sha256, ADR-0030). But a plugin only ever contributes **skills**: of `PluginManager`'s five aggregation methods, exactly one has a non-test consumer — `skill_roots` (`crates/caliban-plugins/src/manager.rs:262` → `caliban/src/main.rs:326` → `startup/compose.rs:632`). `hooks_configs` (`:285`), `mcp_servers` (`:293`), `agent_roots` (`:276`) and `output_style_roots` (`:270`) are parsed, namespaced, `${CALIBAN_PLUGIN_ROOT}`-expanded and then **discarded**; `components.commands` has no aggregation function at all. `compose.rs:1527,1679` both pass `enabled_plugins: &[]` ("empty until ADR 0030 plugin system ships"). The `caliban/src/main.rs:322-324` comment claiming hooks/MCP/agents/styles flow through is inaccurate |
| Plugins bundling browser extensions / scheduled-task templates | 🔴 | no browser-extension or scheduled-task packaging |
| `notify` external-script notifications | 🟡 | **Note corrected 2026-08-15 (#519).** The status-line runner is the only real approximation (`crates/caliban-settings/src/statusline.rs:68` → `caliban/src/tui/app.rs:477`, off-thread refresh). The "hook surface" half of the old note does **not** hold: `Hooks::notification` is declared (`crates/caliban-agent-core/src/hooks.rs:493`) and implemented by the headless sink, but has **no production dispatch site** — the only `.notification(` callers are in `crates/caliban-agent-core/tests/hooks_events.rs`. Still no dedicated `notify` script contract |

## J. Sub-agents / parallelism

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Custom subagent definitions with per-agent model/sandbox/MCP overrides | 🟡 | **Down-ticked 2026-08-15 (#519).** Per-spawn overrides are real, but they come from the `AgentTool` **JSON tool input**, not from any definition file, and the advertised field list was wrong. Honored: `model` (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:63` → `caliban/src/startup/compose.rs:885`), `tool_allowlist` (`:60` → `compose.rs:886-900`), `isolation: worktree` (background path only, `compose.rs:959-962` → `crates/caliban-supervisor/src/server.rs:465-479`), `inherit_hooks`, `inherit_active_mcp`. **Absent entirely: `permissionMode`** (zero Rust hits repo-wide) and **`mcpServers`** (nearest is the boolean `inherit_active_mcp`). No sandbox override either. `maxTurns` is not an input — it is hardcoded `20` (`compose.rs:914`), and `SUB_AGENT_MAX_TURNS` (`agent_tool.rs:21`) is a dead const |
| Subagent file format | 🔴 | **Down-ticked 2026-08-15 (#519) — the old note was factually wrong.** caliban has **no** sub-agent definition file format at all: there is no `.caliban/agents/` or `.claude/agents/` discovery in any Rust code, and `SpawnSpec.frontmatter_path` (`crates/caliban-supervisor/src/proto.rs:95`) is hardcoded `None` at every production construction site (`compose.rs:954`, `caliban/src/agents_cli.rs:323,465`, `caliban/src/tui/events.rs:1008`, `caliban/src/worker.rs:1075`). Codex's canonical TOML (`name`/`description`/`developer_instructions`) is confirmed; the divergence is not Markdown-vs-TOML, it is file-format-vs-none |
| Auto-parallelized delegation, orchestration auto-managed | 🟡 | `AgentTool` + background fleet exist, but fan-out is agent-driven, not an automatic orchestrator |
| Worktree isolation | 🟡 | **Down-ticked from ✅ 2026-08-16 (#555), reconciling with the [Antigravity](../antigravity/parity-gap-matrix.md) §*"Per-agent isolated workspace"* (#554/#559) and [Grok Build](../grok-build/parity-gap-matrix.md) §*"Per-subagent git-worktree isolation"* (#560) matrices, which reached 🟡 on the same code.** The mechanism is real and production-reachable on the **background** path: `isolation: worktree` → `SpawnSpec.isolation_worktree` (`caliban/src/startup/compose.rs:959-962`) → `crates/caliban-supervisor/src/server.rs:465-479` → `worktree_for_agent` → `WorktreeManager` (ADR-0037/0052). **But the documented call gets nothing.** The `AgentTool` schema offers `isolation: "none"\|"worktree"` described as materializing "a dedicated git worktree under .caliban/worktrees/<name>", and **never says `background: true` is also required** (`crates/caliban-tools-builtin/src/agent/agent_tool.rs:201-204`); `background` is a separate key defaulting to **false** (`:67-70`). On the foreground path the factory contains no reference to `isolation`, `worktree` or cwd at all (`compose.rs:884-927`), so a model setting exactly what the schema documents gets a **silent no-op** — the sub-agent runs in the parent's tree believing it is isolated. `caliban agents spawn` hardcodes `isolation_worktree: false` (`caliban/src/agents_cli.rs:474`, also `:328`), so the CLI cannot request one either. And `WorktreeOptions{base_ref, sparse_paths, symlink_directories}` has **no consumer outside its own parse test** (`crates/caliban-tools-builtin/tests/agent_tool.rs:252`): `worktree_for_agent` builds `WorktreeSpec::new(agent_name)` and drops all three (`server.rs:678-684`). Real on one undocumented path, inert on both documented ones — 🟡. Tracked by **#557** |
| Background fleet + supervisor daemon | ✅ | `caliban-supervisor` + `caliband` (ADR-0037) |

## K. Headless / CI

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Non-interactive exec + stderr progress / stdout result | ✅ | `-p` + `--output-format text/json/stream-json` (ADR-0025) |
| NDJSON event stream (`--json`, typed `thread.*`/`turn.*`/`item.*`) | ✅ | `stream-json` NDJSON frames (`system/init`, `message`, `tool_use`, `tool_result`, `result`) |
| JSON-Schema-constrained output (`--output-schema`) | 🟡 | **Note corrected 2026-08-15 (#519) — the ADR pointer was wrong.** ADR-0032 is the OS sandbox; **no ADR covers provider-side structured output**, so this is unspecced, not scheduled. `--json-schema` injects a prompt directive and does a shallow local check of top-level `type`, top-level `required`, and one level of `properties.*.type` (`caliban/src/headless/schema.rs:76-118`) — no `$ref`, nesting, `enum`, or array validation. `response_format` / `json_schema` appear **nowhere** in `crates/caliban-provider*`, so nothing constrains any wire request |
| Stdin piping (`codex exec -`) | ✅ | `--input-format` stdin (10 MiB cap) |
| Env-key auth for CI | ✅ | `ANTHROPIC_API_KEY` / provider env + `--bare` |
| Official GitHub Action (`openai/codex-action`) | 🔴 | GitHub Actions workflow deferred (separate sub-project) |

## L. Observability / cost

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| OpenTelemetry export (`[otel]`) | 🟡 | **Down-ticked 2026-08-15 (#519).** Traces are genuinely ✅ (real `TracerProvider` + `gen_ai` spans, ADR-0053). The other two legs are not: **OTLP logs are absent entirely** — no `LoggerProvider`, log bridge, or logs module anywhere in `crates/caliban-telemetry/`, despite the `opentelemetry-otlp/logs` feature being enabled. **Metrics: 1 of 6 emits from production** — only `emit_session` has non-test callers (`crates/caliban-telemetry/src/init.rs:571,701`); `cost.usage`, `token.usage`, `lines_of_code.count`, `code_edit_tool.decision`, `active_time.total` are exercised only in unit tests, and the four `RECOVERY_*` names are dead constants (#467). Also `enable_telemetry` in settings is inert — the env var is the only switch (#494) |
| Session history log (`history.jsonl`) | ✅ | **Note corrected 2026-08-15 (#519).** Substrate is one pretty-JSON file per session on disk (`FsSessionBackend`, `crates/caliban-sessions/src/backend/fs.rs:12`), selected at `caliban/src/startup/storage.rs:73` — not an append-only `history.jsonl`. `/export [path] [--format json]` writes the transcript (`caliban/src/tui/slash/export.rs:23-71`; the `-` clipboard target is stubbed). The `gonzalo` remote backend is behind an off-by-default cargo feature; `git`/`s3` parse but hard-error as unwired |
| In-session context + rate-limit status (`/status`) | 🟡 | **Down-ticked 2026-08-15 (#519).** `/context` and `/usage` are real and production (`caliban/src/tui/slash/observe.rs:94,26`) — but they report **context and token/cost only**. caliban surfaces **no provider rate-limit status**: the sole `rate_limit` reference is an error-category bucket on the headless event stream (`caliban/src/headless/events.rs:132`). And caliban's own `/status` is a stub that prints the provider name plus "(full provider/auth/subscription status arrives with the Auth spec)" (`caliban/src/tui/slash/model.rs:148-155`) |
| Diagnostics (`codex doctor`) | ✅ | `caliban doctor` / `/doctor` |

## M. Cloud / IDE / long-tail

All 🔴 or **n/a** — large investments, parked until terminal/CLI parity, and
mostly outside caliban's local-first scope. Tracked only so we remember they
exist:

| Capability (Codex) | Caliban | Notes |
|---|---|---|
| Codex Cloud (isolated parallel cloud tasks, delegate-from-CLI) | n/a | no hosted plane; out of scope for the local agent |
| GitHub `@codex` PR review + cloud delegation | 🔴 | no GitHub-app review path |
| IDE extension (VS Code / Cursor / Windsurf / JetBrains) | 🔴 | shared with the Claude Code long-tail (matrix §N) |
| Delegate-to-cloud continuum (local ↔ cloud handoff) | n/a | no cloud plane to hand off to |

---

## Codex-distinctive gaps worth a ticket

Capabilities Codex has that caliban does **not**, and that aren't already
tracked by the Claude Code matrix — the highest-signal candidates if we decide
to chase Codex parity specifically:

1. **`mcp-server` mode** (B/F) — expose caliban itself as an MCP server so other
   agents can drive it. Small, high-leverage, no caliban analogue.
2. **`--output-schema` constrained decoding** (K) — move from best-effort JSON
   validation to provider-native structured output (already gated on ADR-0032).
3. **`ultra`-style auto-delegating effort tier** (G/J) — an effort level that
   automatically fans work out to the background subagent fleet.
4. **Standalone `sandbox` / `execpolicy` subcommands** (B/E) — run/evaluate an
   arbitrary command under a sandbox policy outside a full session.
5. ~~**AGENTS.md as a live, first-class instruction source** (H)~~ —
   **resolved 2026-08-15 (#519); this was never a gap.** `AGENTS.md` is
   already on the live ancestor walk with closer-dir-wins precedence
   (`ANCESTRY_FILENAMES`, `crates/caliban-memory/src/project_walk.rs:42`,
   ADR-0036). Replaced by: **a sub-agent definition file format** (J) —
   caliban has none at all, so every per-agent override must be passed
   inline on each `AgentTool` call.
6. **Config-hook event coverage** (I) — a `[[hooks.*]]` handler can only bind
   to `PreToolUse`/`PostToolUse`/`SessionStart`
   (`crates/caliban-agent-core/src/hooks_router.rs:250`), and
   `allow_managed_hooks_only` disables all config hooks instead of filtering
   by scope (#124). Codex's config-hook surface is wider.

Cloud plane, IDE extension, and GitHub-app review are **deliberately out of
scope** (n/a) — caliban is a local-first terminal agent; do not file these as
parity gaps.

---

## Refresh process

1. When a caliban feature lands: edit the relevant row(s) in this matrix in the
   same PR, ticking 🔴 → 🟡 or 🟡 → ✅.
2. When Codex ships something new: refresh
   [`capability-inventory.md`](capability-inventory.md) first (re-fetch the
   upstream docs + bump the currency marker), then propagate any new rows here.
3. Resolve any **⚠** rows against Codex's live docs / caliban `main` when you
   touch them.
4. Bump the **Last refreshed** date at the top.
