---
title: Permission modes + auto-mode classifier
date: 2026-05-24
status: Proposed
author: john.ford2002@gmail.com
adr: adrs/0029-permission-modes-and-auto-mode.md
---

# Permission modes + auto-mode classifier — Design

**Date:** 2026-05-24
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0029-permission-modes-and-auto-mode.md`

## Goal

Extend caliban's permission model from a single ad-hoc `plan` flag to
the full Claude Code mode set, cycled with `Shift+Tab`:

```
default → acceptEdits → plan → auto → dontAsk → bypassPermissions → default
```

Each mode composes with the existing `Action::{Allow,Deny,Ask}` rule
grammar from `permissions.toml`. The headline addition is **auto
mode**: a classifier-driven policy where a fast model (Haiku-class via
`RequestPurpose::FastClassifier` on the existing model router) labels
each tool call as `allow`/`soft_deny`/`hard_deny`. `soft_deny` falls
through to the Ask modal (ADR 0027); `hard_deny` denies without
prompting.

After this spec ships, operators get: `defaultMode` setting,
`--permission-mode <mode>` CLI flag, `Shift+Tab` cycling with a
status-bar chip, `--allow-dangerously-skip-permissions` gate for
bypass, and an `auto-mode.toml` with curated defaults.

## Non-goals

- **OS-level sandbox.** Tier-4; separate project.
- **TUI Ask modal implementation.** Lives in ADR 0027; consumed here.
- **`/permissions` interactive editor.** Under `/config`+slash work.
- **Live reload of `permissions.toml`.** Config-hierarchy spec's home.
- **Classifier auditing / training-data export.** Out of scope.
- **Per-subagent mode override.** ADR 0021 v2 follow-up.

## Architecture

```
                          tool call dispatched
                                  │
                                  ▼
                  ┌─────────────────────────────────┐
                  │ ModeFilter (NEW)                │
                  │   bypassPermissions latch ────► │ ──► Allow (short-circuit)
                  └──────────┬──────────────────────┘
                             │ otherwise
                             ▼
                  ┌─────────────────────────────────┐
                  │ PermissionsHook (existing)      │
                  │   evaluate(rules) → Allow/Deny/Ask│
                  └──────────┬──────────────────────┘
                             │
                             ▼
                  ┌─────────────────────────────────┐
                  │ ModeFilter post-pass            │
                  │   Allow/Deny pass through       │
                  │   Ask: override by mode         │
                  │     - acceptEdits + file-edit → Allow
                  │     - plan + mutating → Deny
                  │     - auto → classify           │
                  │     - dontAsk → Deny            │
                  └──────────┬──────────────────────┘
                             │
                  ┌──────────┴──────────┐
                  │                     │
              auto mode             other modes
                  │                     │
                  ▼                     ▼
       AutoModeClassifier             final HookDecision
         1. consult auto.toml static rules
         2. call FastClassifier model (router)
         3. cache by (tool,inputHash)
              │
       allow ─┼─ soft_deny ─► Ask modal (ADR 0027)
              │
           hard_deny ─► Deny
```

The static rule layer is unchanged. Modes act as a second filter
running *after* `PermissionsHook::evaluate`. Allow/Deny verdicts pass
through; only `Ask` is mode-overridable — *except* `bypassPermissions`,
which short-circuits everything (including static Deny) and requires
a confirmation flag.

## Crate structure (delta)

```
crates/caliban-agent-core/
└── src/permissions/
    ├── mode.rs           # NEW: PermissionMode enum + SharedPermissionMode
    └── mode_filter.rs    # NEW: ModeFilter composes after PermissionsHook

crates/caliban-auto-mode/ # NEW Layer-3 crate
└── src/
    ├── lib.rs            # re-exports
    ├── config.rs         # AutoModeConfig (TOML loader, $defaults expansion)
    ├── defaults.rs       # curated, version-pinned default rule lists
    ├── classifier.rs     # AutoModeClassifier (consults config + calls router)
    ├── cache.rs          # 256-entry LRU keyed on (tool, sha256(input))
    └── decision.rs       # AutoModeDecision + AutoVerdict

caliban/
├── src/main.rs           # CLI flags (--permission-mode, --allow-dangerously-…)
└── src/tui.rs            # Shift+Tab cycle + chip in status bar
```

`caliban-auto-mode` is Layer-3 so `caliban-agent-core` stays free of
provider-call deps. The binary wires it in by mode.

## `PermissionMode` enum + chip

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default, AcceptEdits, Plan, Auto, DontAsk, BypassPermissions,
}

pub type SharedPermissionMode = Arc<AtomicPermissionMode>;
```

Status-bar chips (rendered next to the existing plan-mode chip):

| Mode               | Chip                  | Color           |
| ------------------ | --------------------- | --------------- |
| `default`          | (none)                | —               |
| `acceptEdits`      | `✎ accept edits`      | green           |
| `plan`             | `📋 plan` (existing)  | cyan            |
| `auto`             | `🤖 auto`             | blue            |
| `dontAsk`          | `⏭ don't ask`        | yellow          |
| `bypassPermissions`| `⚠ bypass`            | red, bold       |

`Shift+Tab` advances; `Shift+Ctrl+Tab` reverses; the cycle wraps. A
toast announces the new mode for 2 seconds.

## Mode semantics

| Mode               | Override applied to verdict from `PermissionsHook`                                    |
| ------------------ | ------------------------------------------------------------------------------------- |
| `default`          | None. Ask verdicts route to `TuiAskHandler`.                                          |
| `acceptEdits`      | If verdict is `Ask` and tool ∈ {Write,Edit,NotebookEdit,MultiEdit}: override Allow.    |
| `plan`             | If tool is mutating (file-edit, Bash, WebFetch): override Deny ("plan mode: read-only"). Existing `SharedPlanMode` flag flips in sync. |
| `auto`             | If verdict is `Ask`: dispatch to `AutoModeClassifier`. Allow/Deny pass through.        |
| `dontAsk`          | If verdict is `Ask`: override Deny ("dontAsk mode: ask suppressed").                   |
| `bypassPermissions`| Short-circuits the *entire* stack to Allow. Requires `--allow-dangerously-skip-permissions` latch.  |

Helpers in `caliban-agent-core::permissions::mode`:

```rust
pub fn is_file_edit(ctx: &ToolCtx<'_>) -> bool;    // Write/Edit/NotebookEdit/MultiEdit
pub fn is_mutating(ctx: &ToolCtx<'_>) -> bool;     // file edits + Bash + WebFetch
```

## `bypassPermissions` gate

The only mode that can override a static `Deny`. To enter it:

- `--allow-dangerously-skip-permissions` at startup sets a
  session-wide `bypass_latch`.
- Cycling via `Shift+Tab` into bypass without the latch fires a
  warning toast and reverts to `default`.
- Starting with `defaultMode = "bypassPermissions"` without the flag
  aborts startup with an explanatory error.

The latch is session-scoped only — no persistence to disk.

## `defaultMode` setting

A new `defaultMode` field in `~/.config/caliban/config.toml` (or
`<workspace>/.caliban/config.toml`) sets the starting mode.
Precedence:

```
CLI --permission-mode  >  project config  >  user config  >  built-in "default"
```

## Auto-mode TOML

```toml
# ~/.config/caliban/auto-mode.toml (or <workspace>/.caliban/auto-mode.toml)

environment = [
  "workspace = $CWD",
  "git_remote = $GIT_REMOTE",
  "is_main_branch = $IS_MAIN_BRANCH",
]

allow = [
  "$defaults.allow",
  "Bash:cargo test*", "Bash:cargo check*", "Bash:cargo clippy*",
  "Read", "Glob", "Grep",
]

soft_deny = [
  "$defaults.soft_deny",
  "Bash:rm *", "Bash:mv *",
  "WebFetch:https://*.internal/*",
  "Write:**/secrets/**", "Write:**/.env*",
]

hard_deny = [
  "$defaults.hard_deny",
  "Bash:sudo *", "Bash:* > /dev/sd*",
  "Bash:rm -rf /*", "Bash:curl * | sh*", "Bash:* | sh*",
  "WebFetch:http://*",
]
```

`$defaults.<list>` expands to a curated, version-pinned list in
`caliban-auto-mode::defaults` (sudo, recursive deletion, piped curl,
secret-bearing paths, plain-http fetches). The `environment` array is
substituted into the classifier's system prompt.

## Classifier protocol

```rust
pub struct AutoModeClassifier {
    config: AutoModeConfig,
    provider: Arc<dyn Provider>,        // router with FastClassifier purpose
    cache: Lru<CacheKey, AutoModeDecision>,   // 256 entries
    disable: bool,                       // disableAutoMode setting
}

pub enum AutoVerdict { Allow, SoftDeny, HardDeny }
pub enum DecisionSource { StaticRule, Classifier, Cached, DisabledFallback }

pub struct AutoModeDecision { pub verdict: AutoVerdict, pub reason: String, pub source: DecisionSource }
```

`classify(&ctx)` flow:

1. **Disable check.** `disableAutoMode = true` or
   `CALIBAN_DISABLE_AUTO_MODE=1` → return `SoftDeny { source:
   DisabledFallback }` (falls through to Ask modal).
2. **Cache lookup.** `CacheKey = (tool_name, sha256(canonical_input))`.
   Hit returns cached decision with `source: Cached`.
3. **Static rule pass.** Walk `hard_deny` → `soft_deny` → `allow` in
   declaration order; first match wins with `source: StaticRule`.
4. **Model call.** `RequestMetadata { purpose:
   Some(RequestPurpose::FastClassifier), .. }` via the router. Prompt
   carries environment context + tool name + 4 KiB-truncated input
   JSON. Strict response schema: `{ "verdict":
   "allow|soft_deny|hard_deny", "reason": "<≤120 chars>" }`.
   Malformed responses → `SoftDeny { reason: "classifier output
   malformed" }`.
5. **Cache write.** Store decision under `CacheKey`. Cache cleared on
   mode exit or `/clear`.

The 4 KiB input truncation prevents prompt blowup; truncation is the
classifier's "best guess" territory and documented as such.

## CLI flags

| Flag                                          | Effect                                                          |
| --------------------------------------------- | --------------------------------------------------------------- |
| `--permission-mode <mode>`                    | Initial mode override. Valid: 6 enum variants in camelCase.     |
| `--allow-dangerously-skip-permissions`        | Session latch permitting `bypassPermissions`.                   |
| `--auto-mode-config <path>`                   | Override auto-mode TOML location (useful in CI).                |
| `--disable-auto-mode`                         | Force `SoftDeny { DisabledFallback }` for every Ask under auto. |

## ModeFilter composition

```rust
pub struct ModeFilter {
    mode: SharedPermissionMode,
    classifier: Option<Arc<AutoModeClassifier>>,
    ask: Arc<dyn AskHandler>,
    inner: Arc<dyn Hooks>,           // typically PermissionsHook
    bypass_latch: bool,
}

#[async_trait]
impl Hooks for ModeFilter {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        if self.mode.load() == PermissionMode::BypassPermissions && self.bypass_latch {
            return Ok(HookDecision::Allow);
        }
        let inner = self.inner.before_tool(ctx).await?;
        self.apply_mode(ctx, inner).await
    }
    // before_turn / after_turn / after_tool delegate to inner
}
```

`apply_mode` matches the table in "Mode semantics" above. Auto-mode's
`soft_deny` builds an `AskRequest` with `comment = "auto-mode
soft_deny: <reason>"` and dispatches to `TuiAskHandler` (ADR 0027) so
the modal renders the classifier's reasoning.

## TUI integration

Status-bar chip rendering reuses the existing render pipeline; the
chip text is `permission_mode.load().chip()`. The plan-mode chip stays
where it is — cycling into `plan` flips the chip and the legacy
`SharedPlanMode` together.

`Shift+Tab` binding in `handle_event`:

```rust
KeyCode::BackTab => {
    let next = app.permission_mode.load().cycle();
    if next == PermissionMode::BypassPermissions && !app.bypass_latch {
        app.toast = Some(Toast::warn(
            "bypass mode requires --allow-dangerously-skip-permissions"));
        app.permission_mode.store(PermissionMode::Default);
        return;
    }
    app.permission_mode.store(next);
    app.toast = Some(Toast::info(format!("permission mode: {}", next.chip())));
}
```

## Sub-agent inheritance

`AgentTool` (ADR 0021) clones the parent's `SharedPermissionMode` into
the child. Per-subagent override is a v2 follow-up.

## Tests (enumerated)

1. **`PermissionMode::cycle`** — six transitions form a cycle.
2. **`from_str` parses all variants** — camelCase round-trips.
3. **`Default` pass-through** — fixture inner returning `Ask`; filter
   returns `Ask`.
4. **`AcceptEdits` overrides Ask for file edits** — Write/Edit go
   Allow; Bash still asks.
5. **`AcceptEdits` preserves static Deny** — Deny on Write stays denied.
6. **`Plan` denies mutating tools** — Bash/Write/WebFetch deny; Read OK.
7. **`Plan` toggle syncs `SharedPlanMode`** — cycling flips legacy flag.
8. **`Auto` calls classifier on Ask** — fixture provider returns
   `{"verdict": "allow"}`; filter returns Allow.
9. **`Auto` soft_deny → Ask modal** — TuiAskHandler receives request.
10. **`Auto` hard_deny → Deny** — verdict carries reason.
11. **`Auto` static rule short-circuits** — `hard_deny = ["Bash:rm -rf
    /*"]`; classifier never called.
12. **`Auto` `$defaults` expansion** — `$defaults.hard_deny` matches
    `Bash:sudo rm`.
13. **`Auto` cache hit** — repeated identical `(tool, input)`; provider
    called once.
14. **`Auto` cache cleared on mode exit** — cycle out of auto and back;
    classifier re-called.
15. **`Auto` disabled fallback** — `disableAutoMode`; returns
    `SoftDeny { DisabledFallback }`.
16. **`Auto` malformed response** — junk JSON → `SoftDeny`.
17. **`Auto` truncates input at 4 KiB** — 100 KiB Edit input; prompt
    body assertion.
18. **`DontAsk` denies Ask** — Bash (Ask) → Deny.
19. **`DontAsk` preserves Allow/Deny** — static rules pass through.
20. **`BypassPermissions` requires latch** — no latch → toast + revert.
21. **`BypassPermissions` overrides static Deny** — latched → Allow
    even for explicit Deny rules.
22. **`--permission-mode acceptEdits`** — CLI parse.
23. **`defaultMode` in project config** — startup picks it up.
24. **`defaultMode = "bypassPermissions"` without flag** — startup
    aborts with expected error.
25. **Subagent inherits parent mode** — AgentTool clones the handle.
26. **`Shift+Tab` cycles in the TUI** — terminal event harness; chip
    updates.

## Risks

- **Classifier latency on the hot path.** Mitigation: 256-entry LRU
  cache, static rule pre-pass, Haiku-class model via the router
  (single-digit-hundred-ms typical).
- **Curated defaults drift.** Mitigation: version-pinned in the crate,
  reviewed each release; soft-deny toasts surface every miss so
  operators learn what to add.
- **bypassPermissions footgun.** Mitigation: red+bold chip, session-only
  latch, warning toast on cycle, prominent README docs.
- **Cache poisoning.** Mitigation: key on canonicalized JSON;
  classifier prompts include a session-rotated salt.
- **Auto+plan collision.** Mitigation: modes are mutually exclusive
  via the cycle; legacy `SharedPlanMode` follows the enum.
- **CI without TTY.** Mitigation: default mode aborts with hint to use
  `dontAsk`; `NonInteractiveAskHandler` continues to back the no-TTY
  path.

## Acceptance criteria

- `cargo build --workspace` clean; clippy clean; fmt clean.
- ≥26 new tests passing.
- `caliban-agent-core::permissions` exports `PermissionMode`,
  `SharedPermissionMode`, `ModeFilter`.
- `caliban-auto-mode` exports `AutoModeClassifier`, `AutoModeConfig`,
  `AutoModeDecision`, `AutoVerdict`, `DecisionSource`.
- Binary accepts the four CLI flags; status-bar chip + `Shift+Tab`
  cycle work in the TUI.
- Both rows under **A. Permissions & safety** in
  `docs/parity-gap-matrix.md` move 🔴/🟡 → ✅ (permission modes with
  Shift+Tab cycle; auto-mode classifier-driven). Sandbox + TUI Ask
  modal rows are handled by other specs.
- README "Permissions" section documents modes, `defaultMode`,
  `auto-mode.toml`, the bypass ceremony, and `disableAutoMode`.
- ADR 0029 in `accepted` status.

## Cross-spec dependencies

- **ADR 0027 (TUI ergonomics)** ships the Ask modal. This spec
  consumes `TuiAskHandler` for auto-mode soft_deny fallthrough.
  **ADR 0027 must merge first.**
- **ADR 0022 (Model router)** already defines
  `RequestPurpose::FastClassifier`. If no route is configured for it,
  startup surfaces a warning and the classifier degrades to
  `DisabledFallback`.
- **ADR 0028 (Checkpointing)** adds `before_run`/`after_run` hook
  events. `ModeFilter` doesn't need them but composes through the
  same `Hooks` trait.
- **ADR 0020 (Permission rules)** is the static-rule layer this
  filter wraps; no API changes.
- **ADR 0021 (Sub-agents)** v1 inherits parent mode by default;
  per-subagent override queued for v2.
