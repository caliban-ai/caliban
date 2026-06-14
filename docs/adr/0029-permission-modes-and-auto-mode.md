# ADR 0029 · Permission modes + auto-mode classifier

- **Status:** accepted
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-permission-modes-design.md`

## Context

caliban's permission model is a static rule grammar
(`permissions.toml`) layered on a single `plan` flag. Claude Code
ships six permission modes cycled with `Shift+Tab`
(`default`/`acceptEdits`/`plan`/`auto`/`dontAsk`/`bypassPermissions`),
each composing differently with the rule grammar. The marquee piece
is `auto` mode, where a fast classifier model labels each tool call
as `allow`/`soft_deny`/`hard_deny` based on workspace/file/network
sensitivity rules.

**A. Permissions & safety** in `docs/parity-gap-matrix.md` flags this
as the headline 🟡/🔴 gap once the OS sandbox is set aside as a
separate Tier-4 investment. ADR 0020 (static rule grammar) and ADR
0022 (model router with `RequestPurpose::FastClassifier`) already
shipped. The infrastructure is in place; this ADR connects the pieces.

The classifier model lives in the router, not the permission system —
the classifier is just another routed call by purpose. The permission
system holds only the orchestration (when to call it, how to cache,
how to compose with static rules).

## Decision

### Permission modes layer over the rule grammar, not under it

The existing `PermissionsHook` continues to produce `Allow`/`Deny`/`Ask`
from static rules. A new `ModeFilter` wraps that hook and overrides
the verdict according to the active mode. Composition order:

```
ModeFilter(BypassPermissions latched) ─ short-circuit Allow
              │ otherwise
              ▼
        PermissionsHook  → Allow / Deny / Ask
              │
              ▼
        ModeFilter post-pass  may override Ask only
```

Static `Allow`/`Deny` always win — operators trust their TOML. Only
`Ask` is mode-overridable, except `bypassPermissions` which
short-circuits everything (including static Deny) and requires an
explicit confirmation flag.

### `bypassPermissions` requires `--allow-dangerously-skip-permissions`

The only mode that can override static `Deny`. To enter it, the
operator must pass the flag at startup (sets a session-wide latch).
Cycling via `Shift+Tab` into bypass without the latch fires a warning
toast and reverts to `default`. Starting with `defaultMode =
"bypassPermissions"` without the flag aborts startup.

### Auto-mode is a classifier consult, cached by input shape

`auto` only runs the classifier when the rule verdict is `Ask`.
Allow/Deny pass through. A 256-entry LRU keyed on `(tool_name,
sha256(canonicalized_input))` caches verdicts for the session. The
classifier dispatches via `RequestPurpose::FastClassifier` on the
existing router — operators wire Haiku, GPT-4o-mini, a local Ollama
model, whatever.

### Static rule pre-pass in `auto-mode.toml`

Before the model call, `auto-mode.toml`'s
`hard_deny`/`soft_deny`/`allow` arrays are walked in that order;
first match short-circuits with `source: StaticRule`. The model is
the expensive fallback, not the first stop. `$defaults.<list>`
expands to a curated, version-pinned default (sudo, recursive
deletion, piped curl, secret-bearing paths, plain-http).

### `soft_deny` falls through to the Ask modal

When the classifier returns `soft_deny`, the verdict becomes a
synthesized Ask request flowing into the same `TuiAskHandler` (ADR
0027) the static `Ask` rules use. The classifier's reason string is
rendered in the modal. This relies on ADR 0027 being merged first.

### A new Layer-3 crate `caliban-auto-mode`

Classifier, config loader, and curated defaults live in a new crate
between `caliban-agent-core` and the router. The core's permissions
module gains only `PermissionMode`, `SharedPermissionMode`, and
`ModeFilter` — provider-call-free types.

Sub-agents inherit parent mode by `SharedPermissionMode` clone (ADR
0021); per-subagent override is v2 follow-up.
`disableAutoMode = true` (or `CALIBAN_DISABLE_AUTO_MODE=1`) is a hard
kill switch — `classify` always returns `SoftDeny { source:
DisabledFallback }`.

## Consequences

- **Positive.** Closes two of three remaining 🔴/🟡 rows under
  Permissions & safety (OS sandbox is deliberately separate).
  Auto-mode is signature differentiation — caliban's
  operator-defined classifier model (any provider) is meaningfully
  more flexible than Claude Code's bundled Haiku. Composition with
  static rules is auditable and testable in isolation.
- **Negative.** One more crate. Hot path gets a network call per
  `Ask` (mitigated by cache + static pre-pass). `bypassPermissions`
  adds a footgun surface needing UX work (red chip, confirmation
  toast). The mode enum overlaps with the existing `SharedPlanMode`
  flag — we keep both for back-compat at the cost of a small
  synchronization burden.
- **Revisit if:** classifier p95 latency becomes a UX problem (could
  pre-compute verdicts for likely next-tool shapes); or if curated
  default lists need more maintenance than the Rust release cadence
  supports (could pull from a versioned upstream JSON).
- **Out of scope, enabled here:** per-subagent permission modes (ADR
  0021 v2), `/permissions` interactive editor, classifier audit log,
  mode-aware hook events (`PermissionRequest`/`PermissionDenied`)
  once the broader hook surface lands.

## References

- Spec: `docs/superpowers/specs/2026-05-24-permission-modes-design.md`
- Static rule layer: `crates/caliban-agent-core/src/permissions.rs`
- AskHandler trait: same file (`AskHandler`, `NonInteractiveAskHandler`)
- FastClassifier purpose: ADR 0022
- Companion ADRs: 0027 (TUI ergonomics — ships Ask modal, must merge
  first), 0028 (Checkpointing — parallel hook-surface work), 0021
  (Sub-agents — v2 refines per-subagent override).
- Parity reference: `docs/claude-code-capability-inventory.md` §6, §3.

## Revised 2026-05-26

The original Decision committed `caliban-auto-mode` to be a new Layer-3
crate. In practice the implementation lives inside `caliban-agent-core`
across `auto_mode.rs`, `mode_filter.rs`, and `permission_mode.rs`
(~1,750 LOC combined).

**Why this is the correct outcome.** Auto-mode dispatch is tightly
coupled to the permission pipeline (`PermissionsHook`,
`SharedPermissionMode`, the soft-deny → Ask handshake) which already
lives in agent-core. Extracting auto-mode would either pull most of the
permission pipeline out with it or introduce a circular dep. The static
rule pre-pass, the classifier dispatch, and the LRU cache all live next
to the data they need.

**Revisit if** auto-mode grows a second consumer (e.g., a non-agent
classifier client), or if the dispatch path becomes a measurable
compile-time burden on `caliban-agent-core`.

## Headless `-p` defaults — what actually runs

When `caliban -p` is invoked without `--permission-mode`,
`--no-permissions`, or any explicit allow/deny/ask flag, the resolved
mode is `PermissionMode::Default` (per `resolve_startup_mode` in
`permission_mode.rs`). Static rule evaluation still runs: the built-in
default-rules tail (`default_rules()` in `permissions.rs`) Allows
read-only tools (`Read`, `Grep`, `Glob`, `TodoWrite`,
`EnterPlanMode`/`ExitPlanMode`), Asks for mutating ones (`Write`,
`Edit`, `Bash`, `WebFetch`), and catch-alls to Ask.

In headless mode, there is no TTY to prompt, so `Ask` verdicts are
routed to `NonInteractiveAskHandler` (in agent-core's
`permissions.rs`). Its behavior:

- `auto_allow: false` (the default) — `Ask` becomes a hard deny. The
  tool call fails with a permission error.
- `auto_allow: true` (set via `--auto-allow` /
  `CALIBAN_AUTO_ALLOW`) — `Ask` becomes Allow. Equivalent to running
  in `dontAsk` mode for the duration of the run.

The net effect: a tool-using prompt that touches only read-only tools
(`Read`, `Glob`, `Grep`) runs to completion silently because each tool
hits an explicit Allow. A prompt that needs `Write`/`Edit`/`Bash`
without `--auto-allow` or an explicit `--allow`/`--permission-mode`
flag will fail on the first such tool call. The lmstudio 2026-05-27
probe (Finding 15) observed the read-only case and reported it as
"auto-dispatch without prompting" — that's accurate, but only because
`Read` is on the default Allow list.

`--no-permissions` is the only way to skip the static rule layer
entirely; the resolved mode surfaces in the `system/init` frame's
`permission_mode` field as the literal string `"disabled"` to make
this state observable (lmstudio Finding 15). All other modes surface
under their camelCase name (`default`, `acceptEdits`, `plan`, `auto`,
`dontAsk`, `bypassPermissions`).
