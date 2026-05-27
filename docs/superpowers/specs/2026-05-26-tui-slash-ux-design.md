# TUI slash & UX polish — Design

**Date:** 2026-05-26
**Author:** john.ford2002@gmail.com
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** *(none yet — propose 0043 if this lands)*
**Origin:** `docs/TODO.md` findings on `/clear` context-tracker reset, `/cost` exposure, real `/doctor` checks, dynamic `/effort`, `/resume` picker, `/context` visualization, `/export`, permission modal "always allow/reject", custom statusline command; plus a follow-up ask to make `/model` a real runtime switch.

## Goal

The parity matrix shows most of these slashes ticked as ✅ because a stub is registered, but the audit found each one is either a static text overlay or a `StatusMessage` placeholder. This spec turns the placeholders into the real things, plus adds two genuinely missing surfaces (`/export`, custom statusline).

After this lands:

1. **`/clear`** also resets the `context_window` tracker so the statusline doesn't lie until the next turn end.
2. **`/cost`** opens an overlay with per-model breakdown + cumulative USD; optional statusline running total.
3. **`/doctor`** runs real checks (settings, providers, MCP, sandbox, stores), reports pass/warn/fail with hints, and is reachable from headless via `caliban doctor`.
4. **`/effort <low|medium|high|max|auto>`** changes reasoning effort at runtime, takes effect on the next turn, shown in the statusline.
4a. **`/model [id|picker]`** switches the active model at runtime, takes effect on the next turn, shown in the statusline.
5. **`/resume [query]`** opens a fuzzy-filterable session picker and swaps the live session in place.
6. **`/context`** opens a real breakdown (stacked bar + top-N largest blocks), with a `--print` headless variant.
7. **`/export [path]`** writes the transcript to markdown (or clipboard, or JSON).
8. **Permission modal** offers `Allow once / Always allow / Reject once / Always reject` — the "Always" branches add a derived rule to the runtime ruleset.
9. **Custom statusline** via `settings.statusLine: { command, timeout_ms }` — claude-code script compatible.

## Non-goals

- **No persistent runtime-rule writes.** "Always allow" lives for the session by default. An optional confirm-to-persist path is mentioned but deferred.
- **No new permission grammar.** The "Always" rules use the existing rule format; only the modal UI changes.
- **No per-session cost persistence.** Cost ledger is per-process today; cross-resume persistence is called out as a follow-up because it requires `caliban-sessions` schema changes.
- **No cross-provider `/model` swap in v1.** The picker lists models from every configured route, but selecting a model whose route uses a *different* provider than the active `Agent` prints a friendly "cross-provider swap not yet supported; restart with `--provider <p> --model <id>`" instead of attempting it. Cross-provider hot-swap needs a `caliban-model-router`-driven Agent factory and is deferred.
- **No new headless output frame types.** `/export --format json` writes a file in caliban's existing session-export shape, not new wire types.
- **No remote-statusline support.** Statusline scripts run locally only; remote execution (e.g. via SSH) is the user's problem.

## Architecture

```
caliban (bin) src/tui/slash/
  basic.rs      ClearCommand               ← UPDATE: reset context_window
  cost.rs       CostCommand                ← NEW
  doctor.rs     DoctorCommand              ← UPDATE: real checks
  effort.rs     EffortCommand              ← UPDATE: real config writeback
  model.rs      ModelCommand               ← UPDATE: real runtime swap + picker
  resume.rs     ResumeCommand              ← UPDATE: picker overlay
  context.rs    ContextCommand             ← UPDATE: real visualization
  export.rs     ExportCommand              ← NEW
  …
  ask.rs        permission modal           ← UPDATE: 4-button variant

caliban (bin) src/main.rs
  + doctor subcommand                       ← NEW headless entry

caliban-agent-core src/config.rs
  + Effort enum + AgentConfig.effort: Arc<ArcSwap<Effort>>
                                            ← lock-free runtime mutation
  AgentConfig.model: String                 ← REMAIN unchanged on the struct shape; but every read site
                                              now goes through Agent::active_model() which returns an
                                              Arc<String> snapshot of an ArcSwap on the Agent itself.
                                              (See "Model swap mechanics" below for the rationale —
                                              the swap lives on Agent, not on AgentConfig, because
                                              AgentConfig is cloned per-turn and ArcSwap-on-clone is
                                              error-prone.)

caliban-provider-openai src/ir_convert.rs
  build_chat_request → reasoning_effort = config.effort.load().as_openai()

caliban-provider-anthropic src/ir_convert.rs
  build_message_request → thinking.budget_tokens = config.effort.load().as_anthropic_budget()

caliban-agent-core src/permissions.rs
  + RuntimeRule (session-scoped, in-memory ruleset alongside config rules)
  + RuleStore::add_runtime_rule(scope, rule)

caliban-settings src/schema.json
  + statusLine: { command: string, timeout_ms?: u32, padding?: u8 }

caliban-statusline (new tiny crate, OR module inside caliban-settings)
  StatuslineRunner::spawn(workspace_root, ctx_json)
    → Result<String, StatuslineError>     ← capped, cached, timed-out
```

## Slash commands

### `/clear` reset (one-line fix)

`caliban/src/tui/slash/basic.rs:25` — `ClearCommand::execute` already clears transcript, messages, TTFT, and session messages. Add:

```rust
ctx.app.context_window.record_history(&[]);
```

before returning `SlashOutcome::Continue`. That's it. Statusline updates immediately.

### `/cost` — new overlay

```rust
struct CostOverlayState {
    grand_total_usd: Decimal,
    per_model: Vec<(String, ModelCost)>,   // (model_id, ModelCost)
    cache_savings_usd: Decimal,
    session_duration: Duration,
}

struct ModelCost {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    usd: Decimal,
}
```

Renders as a fixed-width table (similar shape to existing `/usage` overlay), shown via `SlashOutcome::Overlay(Overlay::Cost)`. Data comes from `caliban-telemetry::CostAccumulator` (existing).

Optional statusline running total: behind `settings.tui.show_cost_in_statusline: bool` (default false). Renders as `$0.0123` in the right segment.

### `/doctor` — real checks

Checklist (each row → `(name, status, hint)`):

| Check | Pass | Warn | Fail |
|---|---|---|---|
| Settings load + parse | All scopes loaded | One unreadable | Effective settings empty |
| Provider auth | All configured providers respond to a cheap ping | One auth missing | All providers auth-fail |
| MCP servers | All servers connected + `list_tools` | One server failed | All servers failed |
| Sandbox detection | Detected mode matches platform | Detected `None` on supported OS | Sandbox required but unavailable |
| Checkpoint store | Writable | Read-only | Directory missing |
| Session store | Writable | Read-only | Directory missing |
| CLAUDE.md ancestors | At least one found | Empty | (no fail case) |

Provider pings cost an API call → gated behind a `--deep` flag (off by default).

Output: a table overlay in TUI, plain text in headless. Headless exit code:
- 0 if all checks pass or warn.
- 1 if any check fails.

CLI: `caliban doctor [--deep]` in `caliban/src/main.rs` (new subcommand alongside the existing `caliban -p` / `caliban` entry).

### `/effort <level>`

Add to `AgentConfig` (in `caliban-agent-core/src/config.rs`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
    Auto,
}

impl Effort {
    /// Maps to OpenAI `reasoning.effort`.
    pub fn as_openai(self) -> Option<&'static str> {
        match self {
            Self::Low    => Some("low"),
            Self::Medium => Some("medium"),
            Self::High   => Some("high"),
            Self::Max    => Some("high"),       // OpenAI caps at "high"
            Self::Auto   => None,               // omit field
        }
    }
    /// Maps to Anthropic `thinking.budget_tokens`.
    pub fn as_anthropic_budget(self) -> Option<u32> {
        match self {
            Self::Low    => Some(2_048),
            Self::Medium => Some(8_192),
            Self::High   => Some(24_576),
            Self::Max    => Some(64_000),
            Self::Auto   => None,
        }
    }
}

// In AgentConfig:
pub effort: Arc<ArcSwap<Effort>>,
```

`ArcSwap` mirrors the existing `permission_mode` shared-state pattern (`caliban/src/tui/permission_mode.rs`). Lock-free reads in the hot path (per-request `build_request`), atomic swaps from the slash handler.

`/effort low` etc. handler:

```rust
async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
    let level = parse_effort(args.trim())?;
    ctx.app.agent_config.effort.store(Arc::new(level));
    Ok(SlashOutcome::StatusMessage(format!("effort → {level}")))
}
```

Statusline chip: when the active model is reasoning-capable (per `Capabilities`), render `⚡L` / `⚡M` / `⚡H` / `⚡Max` / `⚡A` in the same area as the permission-mode chip.

Provider plumbing:
- OpenAI: `crates/caliban-provider-openai/src/ir_convert.rs` reads `agent_config.effort.load()` and sets the JSON field.
- Anthropic: `crates/caliban-provider-anthropic/src/ir_convert.rs` derives `thinking.budget_tokens` (passing `None` omits the thinking block entirely).
- Others: no-op; the field on the request is `Option<>` with `skip_serializing_if = "Option::is_none"`.

### `/model [id|--picker]`

Two invocation modes, sharing one slash:

- **`/model`** (no args) — opens the picker overlay (default UX, mirrors `/resume`).
- **`/model <id>`** — direct switch by exact model id; bypasses the picker. Useful for tab-completion and scripted flows.

**Where the active model lives.** Today `AgentConfig.model: String` is read in three hot places (`stream/mod.rs:255`, `:320`, `:376`). Promoting that field to `Arc<ArcSwap<String>>` would require updating every clone site (the config is `Clone`, and `ArcSwap` is not `Clone`-friendly when you want fork-shared mutation). Instead, hold the swappable model on the `Agent` itself:

```rust
// crates/caliban-agent-core/src/agent.rs
pub struct Agent {
    pub(crate) provider: Arc<dyn Provider + Send + Sync>,
    pub(crate) config: AgentConfig,
    pub(crate) active_model: Arc<arc_swap::ArcSwap<String>>,   // ← NEW; init to config.model
    // …existing fields…
}

impl Agent {
    /// Current model id (lock-free snapshot).
    pub fn active_model(&self) -> Arc<String> { self.active_model.load_full() }

    /// Swap the active model in place. Same-provider only; cross-provider
    /// returns Err with a remediation hint.
    pub fn try_swap_model(&self, new_model: &str) -> Result<(), ModelSwapError> {
        let caps = self.provider.capabilities(new_model);
        if caps.unknown { return Err(ModelSwapError::UnsupportedByProvider(new_model.into())); }
        self.active_model.store(Arc::new(new_model.to_string()));
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModelSwapError {
    #[error("model `{0}` is not available on the active provider")]
    UnsupportedByProvider(String),
    #[error("model `{0}` requires provider `{1}`, but active provider is `{2}`; restart with --provider {1}")]
    CrossProvider(String, String, String),
}
```

Every site that currently reads `self.config.model` is updated to read `self.active_model()` instead. The three identified sites become one-line swaps. `AgentConfig.model` is kept as the *initial* value (used at `Agent::new` to seed `active_model`) — it never changes after construction.

**Picker data source.** Models surfaced in the picker come from `ModelRouter::routes()` (or, if no router is wired, a static fallback list from each provider's `models.rs`). Each row carries:

```rust
struct ModelPickerRow {
    /// Model id as the user types it.
    id: String,
    /// Display name (typically same as id, but can be human-friendly).
    label: String,
    /// Provider that owns this model (e.g., "anthropic", "openai").
    provider: String,
    /// True iff `provider == active_provider`; only these are selectable in v1.
    selectable: bool,
    /// Brief capability summary: "200K context · vision · thinking".
    caps_summary: String,
    /// Cost hint per 1M tokens, formatted ($3.00 / $15.00) if rate-card known.
    cost_hint: Option<String>,
}
```

Cross-provider rows render with a dim style + `[needs restart]` suffix; selecting one prints the `ModelSwapError::CrossProvider` message to the transcript and keeps the overlay open.

**Picker UI** (identical key bindings to `/resume`):
- Up/Down: move selection (skips non-selectable rows by default; `Shift+Up/Down` includes them so the user can preview)
- `/`: focus the filter (fuzzy match against `id`, `label`, `provider`, `caps_summary`)
- Enter: swap; status message `"model → <id>"`; close overlay
- Esc: close without changing

**Statusline integration.** The active model id is *already* the canonical model name in the statusline. Today it's read from `args.model` at startup; with this change the renderer reads `app.agent.active_model()` so the statusline reflects the swap immediately. No new UI affordance — it just stops being stale.

**Provider plumbing.** None needed: `Provider::capabilities(&model_id)` already takes the model as a runtime argument; the per-turn request builder passes whatever model id is current. The cache-marker, context-window, and effort plumbing are all already model-id-driven.

**Argument parsing.**

```rust
async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "--picker" {
        let rows = build_picker_rows(ctx.app);
        return Ok(SlashOutcome::Overlay(Overlay::ModelPicker(ModelPickerState::new(rows))));
    }
    match ctx.app.agent.try_swap_model(trimmed) {
        Ok(()) => {
            ctx.app.context_window.set_capacity(
                ctx.app.agent.provider().capabilities(trimmed).max_input_tokens
            );
            Ok(SlashOutcome::StatusMessage(format!("model \u{2192} {trimmed}")))
        }
        Err(e) => Ok(SlashOutcome::StatusMessage(format!("/model: {e}"))),
    }
}
```

Note the `context_window.set_capacity` call — different models have different context windows, and the statusline's `XX%` only makes sense relative to the new ceiling. Without this, switching from a 200K to a 1M model would show false low utilization.

**Tab-completion** (cheap to add since the slash dispatcher already supports per-command completion hooks if one exists): suggest the same `id`s the picker would show, filtered by prefix.

### `/resume [query]`

Picker overlay listing recent sessions. Already-existing pieces:
- `caliban-sessions` has the list/load surface.
- The TUI already has an overlay convention.

New: a `SessionPickerState` with:
- `sessions: Vec<SessionSummary>` (loaded once on overlay open; sorted by `last_modified` desc; ~50 newest)
- `filter: String` (live fuzzy filter)
- `selection: usize`

Selection swaps session state in place via a new `App::swap_session(SessionSummary)` helper:
1. Cancel any in-flight run.
2. Load `Session` via `caliban-sessions`.
3. Replace `app.messages`, `app.transcript`, `app.context_window` snapshot, `app.session = Some(...)`.
4. Replace cost ledger snapshot (if cross-session persistence lands).
5. Re-render.

Argument hint: `[query]`; aliases `/continue`.

### `/context` visualization

Two views inside one overlay:

**View 1 — stacked bar.** Computed from `app.messages`:

```
Context usage  92K / 200K (46%)
─────────────────────────────────────────────────────────────────────
[sys:  8K ────][user: 12K ────────][asst: 31K ──────────][tool: 41K ─]
```

Each segment color-coded by `MessageKind`: System / User / Assistant text / Assistant tool_use / Tool result.

**View 2 — top-N list:**

```
Largest blocks (descending tokens)
  1. ToolResult  Grep(pattern="ToolResult", path=".")   18.3K  (9.2%)
  2. ToolResult  Read(/home/.../debug.log)              12.1K  (6.1%)
  3. ToolResult  Read(/home/.../config.toml)             8.7K  (4.4%)
  …
```

Tab/key cycles views. Updates live.

Headless variant: `caliban context --print` emits one line:
```
46%: 8K sys / 12K user / 31K asst / 41K tool_result
```

### `/export [path]`

```rust
async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
    let target = parse_target(args, ctx);    // path | "-" | (no arg → default name)
    let format = parse_format(args);          // markdown (default) | json
    let body = render_session(ctx.app, format);
    match target {
        Target::Path(p) => write_file(p, body)?,
        Target::Clipboard => clipboard::set(body)?,
        Target::Default => write_file(default_name(ctx.app), body)?,
    }
    Ok(SlashOutcome::StatusMessage(...))
}
```

Markdown shape:

```markdown
# caliban session 2026-05-26 / <short-id>

- model: anthropic/claude-sonnet-4-6
- duration: 12m 34s
- cost: $0.123 (cache savings $0.045)

## Turn 1 — user
…

## Turn 1 — assistant
…

### Tool: Read
```json
{"file_path": "/Users/.../foo.rs"}
```

```
… result …
```

…
```

`cache_control`, internal IDs, and ephemeral metadata are stripped. JSON format mirrors the on-disk session shape.

### Permission modal — 4 options

`caliban/src/tui/ask.rs` modal state machine adds two new options. Layout:

```
┌──────────────────────────────────────────────────────────┐
│ Bash · "gh pr view 42 --json comments"                   │
│                                                          │
│ This invocation will be allowed by:                      │
│   pattern  Bash(gh pr view *)                            │
│                                                          │
│ (a) Allow once          (A) Always allow this pattern    │
│ (r) Reject once         (R) Always reject this pattern   │
│ Esc — deny                                               │
└──────────────────────────────────────────────────────────┘
```

Pattern derivation (per tool):

| Tool | Derived pattern |
|---|---|
| Bash | `Bash(<first-token> *)` (e.g., `Bash(gh *)`) |
| Edit | `Edit(<workspace-relative-first-segment>/*)` |
| Read | `Read(<workspace-relative-first-segment>/*)` |
| Write | `Write(<workspace-relative-first-segment>/*)` |
| Grep, Glob | `<Tool>(*)` |
| MCP tool | `mcp__<server>__<tool>(*)` |
| Any other | `<Tool>(*)` |

The derived pattern is shown verbatim in the modal so the user is never surprised by what they're allowing.

"Always" branches call `RuleStore::add_runtime_rule(scope=Session, rule=...)`. New `RuntimeRule` is in `caliban-agent-core/src/permissions.rs`; it composes with config rules under the existing precedence (runtime > project > user > managed).

### Custom statusline

`caliban-settings` schema gains:

```json
{
  "statusLine": {
    "type": "object",
    "properties": {
      "command": { "type": "string" },
      "timeout_ms": { "type": "integer", "minimum": 50, "maximum": 5000, "default": 200 },
      "padding": { "type": "integer", "minimum": 0, "maximum": 8, "default": 1 }
    },
    "required": ["command"]
  }
}
```

Runner (in `caliban-settings` or a new tiny `caliban-statusline` crate — TBD per simplicity):

```rust
pub struct StatuslineRunner {
    config: StatuslineConfig,
    cache: Mutex<Option<(Instant, String)>>,   // last-rendered text
}

impl StatuslineRunner {
    pub async fn refresh(&self, ctx: StatuslineContext) -> String {
        // 1. Spawn `command` with workspace as cwd
        // 2. Write `ctx` as JSON to its stdin
        // 3. Wait up to `timeout_ms`; on timeout, return cached text
        // 4. Read stdout, cap to one line and ~120 chars
        // 5. Update cache; return
    }
}
```

`StatuslineContext` is the JSON blob:

```json
{
  "model": "anthropic/claude-sonnet-4-6",
  "cost_usd": "0.1234",
  "permission_mode": "default",
  "effort": "medium",
  "workspace_root": "/Users/.../caliban",
  "session_id": "abc123",
  "turn_count": 7
}
```

Schema matches claude-code's documented contract so existing scripts work unchanged.

Invocation cadence: after every `TurnEnd` / `RunEnd` and on session-load. Never mid-render — uses the cached last value so render is non-blocking. If the script takes >`timeout_ms` for three consecutive turns, log a warning and disable for the rest of the session.

Render placement: as a prefix segment in the existing statusline (before the model/perm/effort chips). One-line max.

## Configuration surface

`caliban-settings` schema additions:

```json
{
  "tui": {
    "showCostInStatusline": { "type": "boolean", "default": false }
  },
  "statusLine": { /* as above */ },
  "effort": {
    "type": "string",
    "enum": ["low", "medium", "high", "max", "auto"],
    "default": "auto"
  }
}
```

Env overrides:
- `CALIBAN_EFFORT=low|medium|high|max|auto` (initial value at startup)
- `CALIBAN_STATUSLINE_TIMEOUT_MS=200`

## Testing strategy

1. **`/clear` reset:** unit test on `ClearCommand::execute` — set `context_window.record_history(&fake)`, run `/clear`, assert `utilization() == 0`.
2. **`/cost`:** seed `CostAccumulator` with two model entries, render overlay, snapshot-compare the rendered table. Round-trip the values through `caliban-telemetry::format_usd`.
3. **`/doctor`:** integration test using a fake settings root + mock MCP servers. Three scenarios: all-pass, one-warn, one-fail. Assert headless exit code matches.
4. **`/effort`:** snapshot test on the slash handler — run `/effort low`, assert `agent_config.effort.load()` returns `Low`. Provider unit test: mock OpenAI ir_convert reads `Low` and sets `reasoning.effort = "low"` in the serialized JSON.
4a. **`/model` swap:** unit test on `Agent::try_swap_model` — happy path (same provider) returns `Ok` and `active_model()` reflects the swap. Cross-provider attempt (a route whose `provider` differs from the active Agent's) returns `ModelSwapError::CrossProvider` and `active_model()` is unchanged. Integration test: dispatch a turn after `/model <id>`, assert the per-turn request was built with the new model id.
4b. **`/model` picker:** with a fake router that returns 3 routes (2 on the active provider, 1 cross-provider), open the overlay, assert all 3 visible, only 2 selectable, the 1 cross-provider row renders dim and selecting it produces a `CrossProvider` status message without changing `active_model`.
5. **`/resume` picker:** with a fake `caliban-sessions` store containing 5 sessions, open the overlay, assert all 5 listed; type "foo" to filter, assert only matching sessions remain. Select one and assert `app.messages` matches the loaded session.
6. **`/context`:** synthesize 20-message history; assert stacked-bar segments sum to total; assert top-N list is sorted desc by token count.
7. **`/export`:** export to a tempdir path; round-trip parse via `pulldown_cmark` and assert structure; export to clipboard mock and assert content present.
8. **Permission modal:** integration test simulating the 4-button flow. After "Always allow", second invocation with the same pattern auto-allows without modal.
9. **Custom statusline:** spawn a fake script that echoes `hello`; assert TUI renders `hello` in the statusline prefix. Spawn a script that sleeps 1s with `timeout_ms=200`; assert cached value is used and a warning logs.

## Telemetry

- `caliban.tui.slash.<cmd>.invoked` (counter) — one per command for usage tracking.
- `caliban.tui.statusline.script_timed_out` (counter)
- `caliban.permissions.runtime_rule_added` (counter, attrs: `tool`, `verdict={allow,deny}`)
- `caliban.tui.effort_changed` (counter, attrs: `from`, `to`)
- `caliban.tui.model_changed` (counter, attrs: `from`, `to`, `same_provider={true,false}`)
- `caliban.tui.model_swap_rejected` (counter, attrs: `reason={unsupported,cross_provider}`)

## Migration notes

- **`AgentConfig.effort`** is new; default `Effort::Auto`. Existing callers compile by adopting the default. Provider crates need a one-line read.
- **`Hooks` trait unchanged.** This spec touches the TUI binary plus three crates (`caliban-agent-core` for `Effort`, `caliban-settings` for schema, providers for plumbing) but no hook-trait shape changes.
- **Runtime rules** are scoped to the session — no on-disk side-effect by default. Users who want persistence get an explicit confirm prompt (deferred to follow-up if it proves friction).
- **Statusline script** runs with the user's full environment by default. Document that scripts have shell-level access. Users who care can set `command: "/bin/sh -c '<script>'"` for explicit subshell isolation; we don't sandbox by default.

## Open questions

0. **Cross-provider hot-swap timeline.** The right architecture is a `caliban-model-router`-driven Agent factory that owns multiple provider instances and swaps the active one. That's a separate spec (model-router v3?). In the meantime, `/model` for cross-provider just nudges the user to restart with new args. Acceptable for v1.
1. **`/cost` vs `/usage`:** the parity matrix already has `/usage` ticked. Do we (a) merge `/cost` into `/usage` as a new tab, or (b) keep them separate? Proposal: keep separate — `/usage` is per-session tokens/turns, `/cost` is dollars. Two muscles, two slashes.
2. **`/export --format json` schema:** does it match the on-disk session format or a slimmer "shareable" shape? Proposal: on-disk format with internal IDs stripped — exact same fields, fewer keys.
3. **"Always" rule persistence:** v1 is session-only. The follow-up question is whether to surface an "[s] save to project" prompt in the modal. Deferred — easier to add than to undo if it turns out to be footgun-y.
4. **Statusline padding:** does `padding` mean spaces or another delimiter? Spaces in v1; YAGNI on other styles.
