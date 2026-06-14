# Slash command coverage — Design

**Date:** 2026-05-24
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `docs/adr/0040-slash-command-registry.md`
**Depends on:** several other 2026-05-24 specs for the commands whose
underlying machinery they introduce (settings, hooks, MCP v2, plugins,
checkpointing, OTel/cost, permission modes, output styles, sub-agents,
auto-memory). This spec adds the *user-facing surface* for those
features.

## Goal

Add the long tail of slash commands caliban needs for parity with
Claude Code, and formalize a `SlashCommand` registry so future commands
plug in alongside the existing `/plan`, `/memory`, `/skills`, `/quit`
without growing `handle_slash_command` into a monster.

This spec is a *bundle*: each command is small in isolation; together
they cover sections K and M of `docs/parity-gap-matrix.md`.

## Non-goals

- **Skill-backed slash commands** (`/code-review`, `/security-review`,
  `/run`, `/verify`, `/batch`, `/loop`, `/debug`, `/ultrareview`). Those
  are *skills*, not commands; they land via the skills system, not
  here. The registry must coexist with them but doesn't define them.
- **Voice command** (`/voice`). Voice dictation is its own sub-project.
- **Theming** (`/theme`). TUI color customization is deferred.
- **`/install-github-app`**, **`/terminal-setup`**, **`/teleport`**,
  **`/remote-control`** — surfaces that interact with hosted Claude
  infrastructure; not relevant to caliban v1.

## Architecture

```
caliban/src/tui/slash.rs           ← new module
  SlashCommandRegistry
    register(name, handler)
    suggest(prefix) -> Vec<&SlashCommandMeta>
    dispatch(name, args, ctx) -> SlashOutcome

  trait SlashCommand: Send + Sync
    fn meta(&self) -> &SlashCommandMeta
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome>

  struct SlashCommandMeta {
    name: &'static str,
    description: &'static str,
    args_hint: &'static str,
    hidden: bool,        // suggest() skips hidden commands
  }

  enum SlashOutcome {
    Continue,                       // returned to input; do nothing
    Quit,                           // exit caliban
    InsertText(String),             // pre-fill the next prompt
    Overlay(Box<dyn Overlay>),      // open an overlay
    Reload,                         // reload settings/skills/hooks/mcp
    StatusMessage(String),          // ephemeral one-line note
  }
```

The TUI's existing `handle_slash_command` becomes a thin wrapper that
calls `SlashCommandRegistry::dispatch`. Each command is its own
`impl SlashCommand` in `caliban/src/tui/slash/*.rs`. The typeahead
suggester in the input bar consults `SlashCommandRegistry::suggest`
instead of a hard-coded list.

## Commands shipped in this PR

Grouped by which underlying machinery they expose. Commands whose
machinery is still being designed in a sibling spec are still defined
here — their `execute` body either no-ops with a "feature not yet
landed" message or proxies to a stub, and gets fleshed out when the
machinery PR lands.

### Session / context

| Command   | Description                                                                                              |
| --------- | -------------------------------------------------------------------------------------------------------- |
| `/clear`  | Clear the current session's message history; keep the system prompt, todos, plan-mode, and skills cache. |
| `/help`   | List all visible registered commands with their descriptions, paginated.                                  |
| `/init`   | Generate a starter `CLAUDE.md` (reads `AGENTS.md`, `.cursorrules`, `.windsurfrules`, the current `git status`, and the local README; produces a draft for the user to edit). |
| `/resume` | Open the session picker overlay (lists `~/.caliban/projects/<...>/sessions/*.json` sorted by mtime, fuzzy-searchable). |
| `/recap`  | Run the Summarizing compactor against the current history and emit the summary as a user-visible message (does not modify the history). |
| `/btw`    | "By-the-way" — run a one-shot ephemeral side question against a fast model (Haiku via the router), no tools, result inlined; doesn't touch the main session. |

### Observability / cost

| Command   | Description                                                                                       |
| --------- | ------------------------------------------------------------------------------------------------- |
| `/usage`  | Show session token usage (input/output/cache_read/cache_creation), estimated $ cost from rate card, and remaining budget if `--max-budget-usd` is set. (Backed by the OTel + cost spec.) |
| `/context`| Show context-window utilization broken down per message; warns at ≥80%.                            |
| `/compact`| Trigger the configured `Compactor` manually; report messages dropped/summarized.                  |
| `/doctor` | Run health checks: settings parse, MCP server reachability, skills loaded, hooks parse, provider auth, workspace permissions; print pass/fail per check. |

### Configuration / extensibility surfaces

| Command       | Description                                                                                         |
| ------------- | --------------------------------------------------------------------------------------------------- |
| `/config`     | Open the tabbed settings editor overlay (Permissions / Hooks / MCP / Memory / UI / Auth tabs; backed by Settings hierarchy spec). |
| `/hooks`      | Open the hooks overlay: list configured hooks per event, show last-fire timestamps, view recent invocation results. |
| `/mcp`        | Open the MCP server status overlay (per MCP v2 spec).                                                |
| `/plugins`    | Open the plugin manager overlay (per Plugin spec): list installed, enable/disable, install from marketplace. |
| `/agents`     | Open the sub-agent fleet overlay (per Sub-agent isolation spec): list foreground/background agents, attach/respawn/rm. |
| `/model`      | Show the current model and the router's per-purpose mapping; tab to switch the primary model.        |
| `/effort`     | Cycle effort level (`low`/`medium`/`high`); takes effect on next assistant turn. (Backed by router v2's effort wiring.) |
| `/status`     | Show auth status for each configured provider (Anthropic API key / OAuth, Bedrock SigV4, Vertex GCP token, Ollama reachable, OpenAI), plus subscription/plan info if known. |
| `/login`      | Run an auth flow for the active provider (Anthropic browser OAuth via `oauth2`+loopback; Bedrock falls back to `aws sso login`; Vertex falls back to `gcloud auth login`). |
| `/logout`     | Clear cached credentials for the active provider.                                                    |
| `/setup-token`| (Anthropic only) Generate a long-lived OAuth token and print it; for CI use.                         |

### Permissions / modes

| Command           | Description                                                                                  |
| ----------------- | -------------------------------------------------------------------------------------------- |
| `/permissions`    | Open the permissions overlay: edit rules in place, see effective rule for a focused tool.    |
| (Shift+Tab cycle) | Cycle permission modes (`default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/`bypassPermissions`); shown in the status bar. (Backed by the Permission modes spec; no slash form.) |

### Diagnostics / dev

| Command       | Description                                                                                                                              |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `/rewind`     | Open the checkpoint picker overlay (per Checkpointing spec).                                                                              |
| `/heapdump`   | Capture a `--features=jemalloc-prof` heap profile if available; otherwise tell the user to rebuild with the feature.                      |
| `/feedback`   | Open a markdown editor; submit posts to a configured endpoint (`feedback_url` setting) — defaults to no-op with a "configure an endpoint" message in OSS builds. |
| `/loop`       | Inline polling: re-invoke the last assistant turn every N seconds until a stop condition; bounded by `--max-turns`. Useful for "wait for CI" workflows. |
| `/statusline` | Customize the status line via a shell-script template (per the Settings spec's `status_line` key).                                        |
| `/tui`        | Toggle fullscreen / default TUI mode.                                                                                                     |
| `/voice`      | *(hidden; reserved for future)* — prints "voice dictation not available in this build."                                                   |

### Already shipped (referenced for completeness; the registry must keep them working)

`/plan`, `/memory`, `/skills`, `/quit`.

## Command file layout

```
caliban/src/tui/
├── slash.rs          # Registry + trait + suggester (~150 LOC)
└── slash/
    ├── basic.rs      # /clear, /help, /quit
    ├── session.rs    # /resume, /recap, /btw, /init
    ├── observe.rs    # /usage, /context, /compact, /doctor
    ├── config.rs     # /config, /hooks, /mcp, /plugins, /agents
    ├── model.rs      # /model, /effort, /status, /login, /logout, /setup-token
    ├── perms.rs      # /permissions
    ├── dx.rs         # /rewind, /heapdump, /feedback, /loop, /statusline, /tui, /voice
    └── stubs.rs      # Commands whose machinery is in another in-flight spec — register, no-op gracefully.
```

Each file's commands are registered in `slash::register_builtin(&mut
registry)` called once from `Tui::new()`. Plugins (per Plugin spec)
register additional commands via `registry.register(...)` during their
load step.

## Suggester behavior

When the input bar's text matches `^/`, the suggester pops a popover
listing visible commands whose name has the typed prefix as a
substring; selection inserts the full name. Same key bindings as the
file-suggestion popover (per TUI ergonomics spec): `Tab`/`↓` next,
`Shift+Tab`/`↑` prev, `Enter` accept, `Esc` dismiss.

## Args parsing

Slash command args are everything after `/<name>` up to the first
newline. Each command parses its own args (most take none). The
registry provides a tiny `parse_kv_args(s) -> HashMap<String, String>`
helper for the few commands that need `--key=value` shape.

## Permissions integration

Slash commands are *not* gated by the permission rule grammar; they're
operator-initiated UI actions, not model-initiated tool calls. The few
that wrap dangerous operations (`/logout`, `/clear`, `/rewind`-restore)
require interactive confirmation in the overlay itself.

## Hooks integration

`UserPromptSubmit` (from the Hooks expansion spec) fires *before* slash
parsing, so a hook can intercept and modify or reject a slash command.
Hook event payload includes `{"prompt": "/clear", "is_slash":
true, "command": "clear", "args": ""}`. Hooks for individual commands
are scoped via `if: "command == 'clear'"`.

## Public API sketch

```rust
// caliban/src/tui/slash.rs

pub struct SlashCommandRegistry {
    by_name: HashMap<&'static str, Arc<dyn SlashCommand>>,
}

impl SlashCommandRegistry {
    pub fn new() -> Self { /* ... */ }

    pub fn register(&mut self, cmd: Arc<dyn SlashCommand>) {
        self.by_name.insert(cmd.meta().name, cmd);
    }

    pub fn suggest(&self, prefix: &str) -> Vec<&SlashCommandMeta> {
        let mut matches: Vec<_> = self.by_name.values()
            .filter(|c| !c.meta().hidden && c.meta().name.contains(prefix))
            .map(|c| c.meta())
            .collect();
        matches.sort_by_key(|m| (m.name.starts_with(prefix) ^ true, m.name));
        matches
    }

    pub async fn dispatch(&self, name: &str, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let Some(cmd) = self.by_name.get(name) else {
            return Ok(SlashOutcome::StatusMessage(format!("unknown command: /{name}")));
        };
        cmd.execute(args, ctx).await
    }
}

#[async_trait]
pub trait SlashCommand: Send + Sync {
    fn meta(&self) -> &SlashCommandMeta;
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome>;
}

pub struct SlashCtx<'a> {
    pub session: &'a mut Session,
    pub registry: &'a SlashCommandRegistry,
    pub config: &'a mut Settings,
    pub providers: &'a ProviderRegistry,
    pub router: &'a Arc<ModelRouter>,
    pub mcp: &'a Arc<McpClientManager>,
    pub skills: &'a Arc<SkillsRegistry>,
    pub hooks: &'a Arc<dyn Hooks>,
    pub fleet: &'a Arc<SubagentFleet>,
}
```

`SlashCtx` is the operator's portal to the running session — commands
need mutable session, immutable references to long-lived registries.
It's intentionally fat: cheaper to thread one struct than to plumb each
field separately.

## Testing strategy

### Unit tests (`caliban/tests/slash/*.rs`)

1. `registry_dispatches_known_command` — `/clear` returns `Continue` and clears history.
2. `registry_unknown_command_returns_status` — `/unknown` returns `StatusMessage`.
3. `suggester_filters_by_prefix` — typing `/co` suggests `/compact`, `/config`, `/context` in that order.
4. `suggester_hides_hidden` — `/voice` not in suggestions.
5. `args_kv_parser` — handles `--key=value`, `--flag`, quoted values.
6. `clear_preserves_skills_and_todos` — after `/clear`, registry + todos intact.
7. `recap_emits_summary_message` — summary content block appended to session log.
8. `usage_renders_zero_when_no_calls` — handles empty session.
9. `context_warns_above_80_percent` — synthetic large history triggers warning glyph.
10. `compact_returns_status_with_count` — message shows "dropped N / summarized M".
11. `doctor_health_checks_run` — each check runs and returns labeled pass/fail.
12. `permissions_overlay_opens` — `/permissions` returns `Overlay`.
13. `help_lists_only_visible` — `/help` excludes hidden commands.
14. `init_creates_claude_md` — runs against a tempdir; CLAUDE.md generated; idempotent (refuses to overwrite without `--force`).
15. `resume_picker_lists_sessions` — given two fake session files, returns both sorted by mtime.
16. `btw_uses_fast_classifier_route` — `RequestPurpose::FastClassifier` stamped on the synthesized request.
17. `loop_respects_max_turns` — bounded by configured cap.
18. `slash_intercepts_via_hook` — `UserPromptSubmit` hook can reject `/clear`.

### Integration

19. `tui_typeahead_suggests_after_slash` — real ratatui test rendering `/c` → popover lists commands.
20. `tui_status_message_appears` — status bar shows ephemeral message returned from a command.

## Risks

- **`SlashCtx` becomes a god-object.** Twelve fields already. If it
  grows past ~20, split into `Read` / `Write` halves.
- **Stub commands lying to the user.** `/usage` before the OTel spec
  lands will say "cost tracking not wired"; risk is the user thinks
  caliban *can't* do it. Mitigation: the stub message names the spec
  doc and ETA.
- **Plugin-supplied commands shadowing built-ins.** Same name → plugin
  loses. Logged at registration time so the operator notices.
- **`/init` overwriting user CLAUDE.md.** Always require `--force` to
  overwrite; default to writing `CLAUDE.draft.md` and prompting.
- **`/login` provider-specific drift.** Auth flow differs per provider
  (browser OAuth, SigV4 cached creds, gcloud token). Each
  `LoginCommand` impl is a small adapter; risk is the adapters become
  thin shells over `cmd!()`; mitigation: each adapter is ≤50 LOC and
  delegates to existing provider crates.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean.
- ≥20 new tests under `caliban/tests/slash/`.
- `SlashCommandRegistry` registered with all 28+ commands above
  (working or stubbed).
- TUI typeahead suggester consults the registry; old hard-coded
  command list removed.
- `/help` lists all visible commands.
- All section M rows in `docs/parity-gap-matrix.md` except the
  skill-backed slash commands (the `/code-review`/`/run`/etc. group)
  move 🔴 → ✅ — those depend on the Skills polish sub-project.
- ADR 0040 in `accepted` status.
