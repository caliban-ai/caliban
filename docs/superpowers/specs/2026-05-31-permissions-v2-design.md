---
title: Permissions v2 — TOML-primary config, schema v2, modal writeback, active management, hardening
date: 2026-05-31
status: Proposed
author: john.ford2002@gmail.com
supersedes_partial: 2026-05-23-permissions-design.md, 2026-05-24-settings-hierarchy-design.md
adr: docs/adr/0034-permissions-v2-and-toml-primary-config.md (to be drafted)
---

# Permissions v2 — Design

**Date:** 2026-05-31
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**Related ADRs:** [0020 — Permission rules](../../../docs/adr/0020-permission-rules.md),
[0026 — Settings layering](../../../docs/adr/0026-settings-layering.md) (refined here),
[0029 — Permission modes + auto-mode](../../../docs/adr/0029-permission-modes-and-auto-mode.md)

## Goal

Make caliban's permission surface genuinely operator-friendly and
genuinely safe. Specifically:

1. **Return caliban's native configuration format to TOML.** JSON is
   reserved for *importing* settings from other agents (Claude Code,
   Codex). This refines ADR 0026 (settings layering) — the layering
   model is correct; the choice of JSON as the canonical *write* format
   was wrong for a Rust project where operators expect TOML and where
   permissions especially benefit from comments and source-order.
2. **Ship a richer per-rule schema** that fixes the source-order loss
   in `permissions.{allow,ask,deny}` arrays, preserves `comment`,
   carries a `reason` field that surfaces to the model on `deny`, and
   adds pattern grammar for the gaps the existing matcher misses
   (globstar paths, "match anywhere" Bash variants, structured MCP
   arguments).
3. **Make the TUI Ask modal's "always allow / always deny" actually
   persist to a file**, with a scope picker (session / local / project
   / user) and a "narrow this rule" sub-prompt that defends against the
   classic `Bash:cargo *` over-allow footgun. This delivers on the
   promise in the original ADR 0020 modal that the v1 implementation
   silently dropped.
4. **Give operators a complete active-management surface** — a `/permissions`
   TUI editor that can add/edit/delete/promote rules with a write-scope
   picker, plus a `caliban perms` CLI with `list / test / explain / add /
   remove / import / export / audit` subcommands for headless and
   scripted use.
5. **Add hardening primitives** — a `permissions.enforce` lockdown knob
   that a managed or project file can use to ban `--no-permissions` and
   bypass mode; a persistent decision log for after-the-fact auditing;
   modal UX that teaches narrower patterns; an always-visible bypass
   chip + drop-bypass keybind.

## Non-goals

- **OS-level sandbox.** Tier-4 separate sub-project (ADR 0032).
- **Classifier-based approval at the static-rule layer.** Auto-mode
  classifier already lives in ADR 0029; v2 doesn't change it.
- **Per-subagent rule overrides.** Subagents inherit the parent's
  rule set and `SharedPermissionMode`; per-subagent diff is a v3
  follow-up.
- **Time-bounded rules ("allow for 10 minutes").** Deferred — easy to
  add later via an `expires_at` field on `[[permissions.rules]]`.
- **Signed / cosign'd rule bundles.** Out of scope; the file-mode
  permission model of `permissions.toml` is sufficient for v2.
- **Capability tokens, per-host network ACLs as a separate DSL.**
  Network rules continue to ride on `WebFetch:<url-glob>`.
- **Rewriting the JSON Schema publication story.** We continue to
  publish JSON Schema for editors, because editors can still produce
  JSON imports caliban consumes; we additionally publish a taplo
  schema so TOML editors get matching autocomplete.
- **Real-time rule-shadowing linter.** A `caliban perms lint` is
  hinted at in the CLI but its analysis depth is out of scope for v2.

## Problems this spec solves

Drawn from a fresh read of `caliban-agent-core::permissions`,
`caliban-agent-core::permission_mode`, `caliban-settings`,
`caliban/src/tui/ask.rs`, and the existing specs / ADRs.

### Correctness / broken promises

1. **The TUI Ask modal's `AlwaysAllow` / `AlwaysReject` does not
   persist.** `caliban/src/tui/ask.rs:60` documents that both branches
   only append to the session-scoped `RuntimeRuleStore`. ADR 0020 §
   "Interactive Ask flow" promised file-backed persistence ("Y allow
   permanently…appends a rule to the user file"). Operators reasonably
   believe their sessions are hardening their settings; they are not.
2. **JSON `permissions.{allow,ask,deny}` loses source order.** The
   converter at `caliban-settings/src/settings.rs:304` flattens the
   three arrays to a single rule list with a hardcoded `deny > ask >
   allow` precedence. The legacy TOML `[[rule]]` form preserved
   source order. A user who wants "deny `Bash:*` except `Bash:git *`"
   cannot express that today: `deny` always wins.
3. **`comment` is silently dropped on legacy → JSON migration.** The
   TOML `[[rule]]` schema carries `comment`; the JSON three-bucket
   form has no place for it. Migration loses operator intent.
4. **Configuration polarity drift.** `caliban-settings` was authored
   JSON-first to chase Claude Code parity. This is the wrong polarity
   for a Rust project; comments, source-order semantics, and
   hand-editing all suffer. ADR 0026 calls this out under "Risks"
   ("TOML/JSON drift") but accepted the polarity nonetheless. v2
   inverts it.

### Schema gaps

5. **No globstar `**`.** The hand-rolled matcher (`*` and `?`) cannot
   express `Edit:src/**/*.rs`. Operators write what they think will
   work and the rule silently never fires.
6. **No path normalization.** `Edit:./src/foo.rs` matches the literal
   string `./src/foo.rs`. A tool invocation with an absolute
   `file_path` does not match. Same rule written two ways behaves
   differently.
7. **No MCP argument scoping.** First-arg accessors are closed:
   `Bash → command`, `WebFetch → url`, `Read/Write/Edit → file_path`,
   everything else → none. With 30+ MCP tools accepting structured
   input, the floor for MCP rule granularity is the tool name. There
   is no way to write "allow `mcp__github__create_issue` only for the
   `anthropic/*` org".
8. **No "match anywhere in the command" variant for Bash.** `Bash:rm
   *` famously does not match `sudo rm`, `\rm`, `command rm`, or
   `bash -c "rm …"`. Documented in ADR 0020 risks, never mitigated.
9. **No `reason` field that surfaces to the model on deny.** The
   model receives a generic "permission denied for tool 'X'" string
   today. Operators can't communicate "the human said no; here's
   why, try another approach" without it landing in the comment that
   never reaches the model.

### Active-management gaps

10. **`/permissions` overlay is read-only for config rules.** PR #82
    shipped view-mode + runtime-rule editing. To add a project rule
    operators must exit the TUI and hand-edit a file. There is no
    "promote this runtime rule to project / user file" action.
11. **No `caliban perms` CLI.** No `list`, `test`, `explain`, `add`,
    `remove`. Headless and scripted workflows have no rule-management
    surface.
12. **No "why did this match?"** The matcher *internally* knows which
    rule fired (`evaluate_with_rule` returns it) but the information
    is not surfaced to the operator on success.
13. **Runtime rules evaporate silently on restart.** A 30-minute
    session can accumulate 20 runtime rules that vanish with no
    "save these to your project file?" prompt at shutdown.
14. **No rule preview.** Operators cannot ask "would `Bash:cargo *`
    match `cargo install --git ...`?" without firing the tool.

### Security / hardening gaps

15. **`derive_pattern` teaches weak patterns.** For
    `Bash:cargo test`, the suggestion is `Bash:cargo *` — one Y-press
    grants every `cargo install <anything>` for the rest of the
    session. The modal needs to expose narrower defaults before
    commit.
16. **No persistent decision audit log.** ADR 0020 listed this as a
    risk and explicit follow-up. Two years of releases later, an
    operator still cannot answer "what dangerous things ran in last
    week's sessions?"
17. **No `permissions.enforce` lockdown.** A managed or project file
    cannot ban `--no-permissions` or bypass mode. Compliance and team
    policy use cases have no enforcement point.
18. **`bypass_latch` is silent and irrevocable mid-session.** Once
    `--allow-dangerously-skip-permissions` is on, the only signals
    are the chip (when in bypass mode) and the original startup
    toast. There is no way to drop the latch without restart.

## Architecture

```
                       file watcher (notify, debounced 250ms)
                                        │
   ┌──────────────────────────────────────────────────────────────────┐
   │                  ScopeLoader (TOML-primary)                      │
   │   Reads `<scope>/settings.toml` first; `<scope>/settings.json`   │
   │   only when no .toml present (legacy/import). WARN on .json.     │
   │   Returns Option<Settings>.                                      │
   └────────────────────────────────┬─────────────────────────────────┘
                                    │
        ┌────────┬──────────────────┼──────────────────┬────────┐
        ▼        ▼                  ▼                  ▼        ▼
     Managed   User              Project             Local     CLI
                                    │
                                    ▼
                         SettingsMerger (unchanged)
                                    │
                                    ▼
                          Settings (ArcSwap)
                                    │
        ┌──────────────────────────┼────────────────────────────┐
        ▼                          ▼                            ▼
PermissionsHook +             /permissions overlay      caliban perms CLI
 ModeFilter                   (TUI editor, write-scope) (subcommands)
        │                          │                            │
        │                          └─── atomic TOML writes ─────┤
        │                               (per-file flock)        │
        ▼                                                        ▼
RuntimeRuleStore  ◄─── runtime-rule writes ─────────  caliban perms add
       │                                                 (writes TOML)
       ▼
DecisionRecorder (append JSONL)
       │
       ▼
$XDG_STATE_HOME/caliban/permission-decisions.jsonl
```

Key compositions:

- The **glob matcher / `PermissionsHook` / `ModeFilter` evaluation
  path is unchanged structurally.** What changes is the *rule schema*
  it consumes, the *pattern grammar* it recognizes, and the *sources*
  it draws from.
- The **runtime rule store is preserved.** It is now joined by an
  on-disk `caliban-settings` write surface for the modal's "save to
  file" branches.
- **DecisionRecorder is a thin new `Hooks` impl** that wraps
  `PermissionsHook`, captures each `Allow`/`Deny` outcome, and
  appends a JSONL record. It is composed via the existing
  `CompositeHooks` chain — no new core trait.

## Crate-level changes

```
crates/caliban-settings/
├── src/
│   ├── loader.rs          # MODIFIED: TOML-primary dispatch; JSON triggers a
│   │                      #  one-line WARN on first read; canonical
│   │                      #  filenames become `settings.toml`,
│   │                      #  `permissions.toml`, etc.
│   ├── writer.rs          # NEW: atomic TOML write helpers (tmp+rename,
│   │                      #  per-file fs2::FileExt::lock_exclusive flock,
│   │                      #  source-preserving edits where possible)
│   ├── settings.rs        # MODIFIED: Permissions struct gains
│   │                      #  `rules: Vec<RuleSpec>` (canonical), `enforce`,
│   │                      #  `default_mode`, `audit_log`. Legacy `allow`/
│   │                      #  `ask`/`deny` fields kept for input compat,
│   │                      #  flattened to `rules` on load.
│   ├── import.rs          # NEW: detect-and-flatten for Claude Code JSON,
│   │                      #  Codex JSON, legacy caliban JSON. Output is
│   │                      #  always a TOML-shaped Settings value.
│   ├── schema.rs          # MODIFIED: still emit JSON Schema; additionally
│   │                      #  emit a taplo schema (TOML editor autocomplete).
│   └── compat.rs          # MODIFIED: legacy `permissions.toml` loader
│                          #  continues; demoted from "primary" to "legacy";
│                          #  emits DEPRECATED WARN once per process.
│
crates/caliban-agent-core/
└── src/
    ├── permissions.rs     # MODIFIED: pattern grammar grows globstar `**`,
    │                      #  path normalization, `~`-prefix "match anywhere"
    │                      #  for Bash, dotted-key MCP arg accessors. Rule
    │                      #  struct gains `reason` (deny only). Runtime
    │                      #  store unchanged.
    ├── permission_mode.rs # MODIFIED: `enforce` settings handling at startup
    │                      #  (refuse `--no-permissions`, bypass).
    └── decision_log.rs    # NEW: `DecisionRecorder` Hooks impl + JSONL writer
                           #  with size/age rotation.
│
caliban/
├── src/
│   ├── tui/
│   │   ├── ask.rs         # MODIFIED: `AlwaysAllow`/`AlwaysReject` open a
│   │   │                  #  sub-prompt for narrow-rule + scope picker;
│   │   │                  #  writes via caliban-settings::writer.
│   │   ├── permissions_overlay.rs  # MODIFIED: full editor — sections by
│   │   │                  #  source, `[a]/[e]/[d]/[p]romote/[t]est` keys,
│   │   │                  #  write-scope picker.
│   │   └── bypass_chip.rs # NEW (or in tui/status_bar.rs): chip stays
│   │                      #  visible all session when latched; drop-bypass
│   │                      #  keybind `Ctrl+Shift+B`.
│   └── perms_cli.rs       # NEW: `caliban perms` subcommand surface.
```

New workspace dep:

```toml
fs2     = "0.4"   # cross-platform exclusive file locks for atomic writes
```

`schemars` (JSON Schema) and `taplo`-style schema export stay as
existing deps; no new TOML-specific parser (the `toml` crate already
in the workspace handles read + write).

## TOML polarity flip

### Canonical filenames

| Scope     | Linux                                                     | macOS                                                                          | Windows                                                              |
|-----------|-----------------------------------------------------------|--------------------------------------------------------------------------------|----------------------------------------------------------------------|
| Managed   | `/etc/caliban/managed-settings.toml` + `managed-settings.d/` | `/Library/Application Support/Caliban/managed-settings.toml` + `managed-settings.d/` | `C:\ProgramData\Caliban\managed-settings.toml` + `managed-settings.d\` |
| User      | `$XDG_CONFIG_HOME/caliban/settings.toml` (default `~/.config/caliban/settings.toml`) | `~/Library/Application Support/Caliban/settings.toml` (also reads `~/.config/caliban/settings.toml`) | `%APPDATA%\Caliban\settings.toml` |
| Project   | `<cwd>/.caliban/settings.toml`                            | same                                                                           | same                                                                 |
| Local     | `<cwd>/.caliban/settings.local.toml`                      | same                                                                           | same                                                                 |

Permissions get a dedicated file when the operator prefers
separation: each scope additionally accepts a sibling
`permissions.toml` whose contents merge into `settings.toml`'s
`permissions` table at the same scope. Same applies to `hooks.toml`
and `mcp.toml`. The split-file form mirrors Cargo's
`Cargo.toml` + `.cargo/config.toml` pattern and is the recommended
form for large rule sets.

### Read precedence

Inside a single scope:

1. `settings.toml` and (if present) `permissions.toml`/`hooks.toml`/`mcp.toml`
   merge into one effective in-memory scope (top-level keys from the
   per-feature file override the matching top-level key in
   `settings.toml`, on the theory that the operator who chose the
   split form wants that file to be authoritative for its slice).
2. `settings.json` is read only when **no** TOML file exists in this
   scope. A one-line WARN fires at startup
   (`settings.json detected at <path>; this is a legacy/import path.
   Run `caliban settings import --from <path>` to migrate.`).
3. If both `.toml` and `.json` exist, `.toml` wins, `.json` is
   ignored, WARN fires once.

### Write precedence

caliban-owned writes (modal "always allow", `/permissions` overlay
edits, `caliban perms add`, etc.) **only ever write TOML**. There is
no code path in the entire system that produces a `.json` settings
file. Atomic writes go through `caliban-settings::writer`:

1. Acquire `fs2::FileExt::lock_exclusive` on the target file (create
   if missing).
2. Read current contents (empty if missing), parse, apply mutation
   (append a `[[permissions.rules]]`, set a scalar, etc.).
3. Write the new contents to `<target>.tmp` in the same directory
   (sibling temp file → same filesystem → rename is atomic).
4. `fsync`.
5. `rename(<target>.tmp, <target>)`.
6. Release the lock.

The file-watcher fires after the rename and the running process
picks up its own edit through the normal live-reload path.

### Schema publication

- JSON Schema continues to be emitted at build time to
  `target/schemas/caliban-settings.json` and published to
  `https://caliban.dev/schemas/settings.json`. This serves *editor
  autocomplete for JSON imports* and documents the type surface for
  third parties consuming caliban configs.
- A new build-step emits a taplo schema to
  `target/schemas/caliban-settings.taplo.toml` and publishes to
  `https://caliban.dev/schemas/settings.taplo.toml`. Operators add
  `# taplo: schema = "https://caliban.dev/schemas/settings.taplo.toml"`
  to the top of their `settings.toml` for in-editor schema-driven
  autocomplete.

## Permissions v2 schema (TOML)

### Canonical example

```toml
# ~/.config/caliban/permissions.toml
# Or: place the same [permissions] table under settings.toml.

[permissions]
# When true: --no-permissions, bypass mode, and `--auto-allow` are all
# refused at startup. Set from managed or project scope for compliance.
enforce = false

# Initial mode at session start; CLI --permission-mode wins, env wins
# over file. Valid: default / acceptEdits / plan / auto / dontAsk /
# bypassPermissions (bypassPermissions requires
# --allow-dangerously-skip-permissions, even from this file).
default_mode = "default"

# Append-only JSONL decision log at $XDG_STATE_HOME/caliban/.
# Default true. Set false to silence (cost: cannot audit later).
audit_log = true

# Rules are an ordered array. First match wins, top to bottom. No
# bucket-level precedence. Comments survive. Operator intent is the
# source of truth.
[[permissions.rules]]
pattern = "Bash:git *"
action  = "allow"
comment = "git ops are fine"

[[permissions.rules]]
pattern = "Bash:rm *"
action  = "deny"
reason  = "I want to use git revert or the Write tool instead."

[[permissions.rules]]
# `~`-prefix: "match anywhere in the command line", catches `sudo rm`,
# `bash -c "rm …"`, `command rm`, etc.
pattern = "Bash:~rm *"
action  = "deny"
reason  = "Even via a wrapper, rm is too easy to misuse."

[[permissions.rules]]
# Path globstar; matched against the workspace-normalized form of
# `file_path`. Equivalent to `Edit:**/*.md` from any cwd in the repo.
pattern = "Edit:**/*.md"
action  = "allow"

[[permissions.rules]]
# Dotted-key MCP arg matching. Multiple `key=glob` pairs comma-
# separated AND together.
pattern = "mcp__github__create_issue:repo=anthropic/*,title=*"
action  = "allow"

[[permissions.rules]]
pattern = "WebFetch:https://*.internal/*"
action  = "deny"
reason  = "Internal domains require an explicit ask each time."

[[permissions.rules]]
pattern = "*"
action  = "ask"
```

### Per-rule fields

| Field     | Required | Type     | Notes                                                                                  |
|-----------|----------|----------|----------------------------------------------------------------------------------------|
| `pattern` | yes      | string   | See "Pattern grammar" below.                                                            |
| `action`  | yes      | string   | `"allow"` / `"ask"` / `"deny"` (case-insensitive on read; lowercase on write).         |
| `comment` | no       | string   | Free text; shown in the Ask modal + decision log; not seen by the model.               |
| `reason`  | no       | string   | Deny-only; shown to the model in place of the generic "permission denied" message.     |
| `expires_at` | no    | datetime | RFC3339; v2 reserves the field but does not honor it (deferred to a "time-bounded" v3). Unknown fields are a parse error today, so we reserve this field now to make the v3 addition non-breaking. |

Any other field is a parse error at load time (caught by `toml::de`
with line+col); v2 still uses `#[serde(deny_unknown_fields)]` for
forward-incompatible safety.

### Pattern grammar

Grammar (informal EBNF):

```
pattern   ::= tool_pat [":" arg_spec]
tool_pat  ::= glob              ; over the tool name
arg_spec  ::= bash_anywhere | path_glob | kv_specs | first_arg_glob

bash_anywhere ::= "~" glob       ; only valid when tool_pat resolves to "Bash"
path_glob ::= glob               ; with `**` enabled, for tool_pat in
                                ; { Read, Write, Edit, MultiEdit, NotebookEdit }
                                ; path is workspace-normalized before matching
kv_specs  ::= kv ("," kv)*
kv        ::= dotted_key "=" glob
dotted_key ::= ident ("." ident)*
first_arg_glob ::= glob          ; fallback for tools with a closed
                                 ; first-arg accessor (Bash command,
                                 ; WebFetch url, etc.) and no `~`/`=`
                                 ; markers

glob      ::= chars-and-wildcards using `*`, `?`, `**`
```

Pattern recognition rules:

- **`tool_pat`** matches the tool name as a glob. `*` matches every
  tool (used by the catch-all default rule).
- **`Bash:~<glob>`** matches when `<glob>` matches *any contiguous
  substring* of the Bash `command` field. This is what operators
  intuitively expect from "deny rm" — it should catch `sudo rm`.
  Example: `Bash:~rm *` matches `rm -rf node_modules`, `sudo rm
  …`, `bash -c "rm -rf foo"`. Available only for `Bash` to keep the
  grammar simple; if pressed for other tools we can broaden.
- **Globstar `**`** is recognized in every pattern (no
  tool-class restriction); operators reasonably expect it
  everywhere. For file-edit tools (`Read`/`Write`/`Edit`/`MultiEdit`/
  `NotebookEdit`) the matcher additionally applies *path
  normalization* before running the glob: a pattern starting with
  `/` matches absolute paths literally; a pattern starting with
  `./` or `..` or no leading separator is interpreted relative to
  the workspace root (the discovered git toplevel, or the cwd if
  not in a git checkout). The matched path is itself
  workspace-normalized before the glob runs, so `Edit:src/**/*.rs`
  and the same rule spelled `Edit:./src/**/*.rs` are identical.
  For non-path tools, `**` behaves as a glob over the raw
  first-arg string (with `**` matching across slashes if any are
  present — a reasonable default for URL globs in `WebFetch`). The
  matcher uses `globset::Glob` for both the path-glob and
  non-path-glob paths so the syntax is uniform.
- **Dotted-key MCP args (`key=glob` or `outer.inner=glob`)** look up
  the value in the tool input JSON. Unknown keys (or non-scalar
  values) compare against the empty string — so `key=*` matches an
  unknown key (per glob semantics) but `key=foo*` does not. Multiple
  pairs comma-separated are AND-combined. This unblocks targeted
  MCP rules like `mcp__github__create_issue:repo=anthropic/*`. The
  comma is *not* legal inside a glob; if an operator needs a literal
  comma the existing `?` single-char wildcard handles it.
- **`Tool:<glob>`** with no `~`, no globstar in a path tool, no `=`
  signs preserves v1 behavior: matches the closed-set first arg
  (`command`/`url`/`file_path`) as a glob. Backward-compatible with
  every v1 rule.

### Migration of existing forms

- **Legacy TOML `[[rule]] tool=… action=… comment=…`** continues to
  load. `tool` is aliased to `pattern`. On any caliban-owned write
  to the same file, the file is *rewritten in canonical form*
  (`[[permissions.rules]] pattern=…`), preserving comments and rule
  order. A one-time DEPRECATED WARN fires per process when the
  legacy form is read.
- **Legacy JSON `permissions.{allow,ask,deny}: []`** continues to
  load. The three arrays are concatenated into the ordered `rules`
  array in the order `deny → ask → allow` (so the v1 behavior is
  preserved bit-for-bit for any operator who relied on it). The
  legacy JSON file is *never written back*; the next caliban-owned
  edit emits a sibling `.toml` and the WARN telling the user to run
  `caliban settings import`.
- **JSON imports from other agents** (Claude Code's `settings.json`,
  Codex's `config.json`) go through the same flatten path, with
  agent-specific shape detection. `caliban settings import --from
  <path>` makes this an explicit operator gesture.

## Modal writeback (P1)

### Flow

When the operator presses `Y` (always allow) or `N` (always deny) in
the Ask modal, the modal opens a sub-prompt before committing
anything. No file write happens without explicit confirmation.

```
┌─ Always allow / deny ────────────────────────────────────────────────┐
│ Pending tool call:                                                    │
│   Bash                                                                │
│     command: "cargo test --all"                                       │
│                                                                       │
│ Suggested patterns (default selection is the narrowest):              │
│   ( ) Bash:cargo *               allow any cargo invocation           │
│   ( ) Bash:cargo test*           allow cargo test variants only       │
│   (•) Bash:cargo test --all      exact match only                     │
│   ( ) [custom...]                edit pattern + see live preview      │
│                                                                       │
│ Save to:                                                              │
│   ( ) session     in-memory; gone on restart                          │
│   (•) project     .caliban/permissions.toml; commit-friendly          │
│   ( ) user        ~/.config/caliban/permissions.toml; this user only  │
│   ( ) local       .caliban/permissions.local.toml; gitignored         │
│                                                                       │
│ Optional comment: ______________________________________              │
│                                                                       │
│ [enter] save  [esc] cancel and allow this one call only               │
└──────────────────────────────────────────────────────────────────────┘
```

Behavior details:

- **Suggested patterns** are derived from the tool call. For Bash:
  the first token (`cargo`), the first two tokens (`cargo test`),
  the literal full command. For path tools: the file's directory,
  the file's parent directory plus `**`, the literal file path. For
  MCP tools: tool name only, then each scalar input key turned into
  a `key=value` (literal) suggestion. Order: broadest → narrowest.
  Default selection: the *narrowest* option. This is a deliberate
  inversion of the v1 modal's implicit "broad" default — v2 makes
  narrow the default so an operator who hammers enter gets a tighter
  rule than they would have today.
- **`[custom…]`** opens a single-line editor with a live preview
  line under it (`Would match this pending input? yes/no`), updated
  on every keystroke. The matcher runs against the pending tool
  input only — narrowing is the only safe direction at this stage.
- **Save to** defaults to project. Picker is keyboard-driven (arrow
  keys / `j`/`k`). Saving to session uses the existing
  `RuntimeRuleStore::add` and produces no file write.
- **Comment** is optional; if non-empty, written into the
  `[[permissions.rules]].comment` field on file save.
- **`esc`** is a clean cancel: no rule is written, the pending tool
  call is allowed once (the equivalent of the `y` / "allow once"
  branch). This is documented prominently so the modal is never a
  one-way commit.
- **Atomic write** uses `caliban-settings::writer` (see "Write
  precedence" above). The watcher picks up the change and the rule
  set updates immediately.

The `N` / "always deny" branch follows the same flow with `action =
"deny"`; the modal additionally surfaces an optional `reason:` field
that surfaces to the model when the rule fires.

### Concurrent-modal safety

Two concurrent `Ask` prompts firing simultaneously (rare but
possible under parallel tool dispatch — ADR 0021) cannot both write
to the same `permissions.toml` because of the `flock`. The second
write waits, then re-reads, applies its mutation on top of the first
write, and renames. No silent lost-update.

If two *distinct* caliban processes (e.g. a CLI run and a TUI
session) edit the same file simultaneously, the watcher in the loser
picks up the winner's change and the loser's UI shows a "rule list
changed on disk; reload?" toast before its next write.

## `/permissions` TUI editor

The overlay grows from view-mode + runtime-rule deletion (PR #82) to
a full editor.

```
┌─ /permissions ────────────────────────────────────────────────────────┐
│ View(▶)  Edit  Audit                                                  │
├───────────────────────────────────────────────────────────────────────┤
│ Source filter:  ─ all ─ session ─ local ─ project ─ user ─ managed ─  │
│                 ─ default                                              │
├───────────────────────────────────────────────────────────────────────┤
│ #  Source   Pattern                              Action  Comment       │
│ 1  session  Bash:cargo *                         allow   (modal Y)     │
│ 2  project  Bash:git *                           allow   git ok        │
│ 3  project  Bash:rm *                            deny    no rm         │
│ 4  project  Bash:~rm *                           deny    no rm anywhere│
│ 5  user     Edit:**/*.md                         allow                 │
│ 6  user     mcp__github__create_issue:repo=…/*   allow                 │
│ 7  default  Read                                 allow                 │
│ …                                                                      │
│ N  default  *                                    ask                   │
├───────────────────────────────────────────────────────────────────────┤
│ [a]dd  [e]dit  [d]elete  [p]romote→file  [t]est  [/] filter  [enter]   │
│ [↑↓] move  [esc] close                                                │
└───────────────────────────────────────────────────────────────────────┘
```

Keys:

- **`a`** — add a new rule. Opens the same sub-prompt as the modal
  writeback (pattern, action, comment, scope picker). Pattern starts
  empty; no live-preview unless the operator enters a pending input
  via `[t]est`.
- **`e`** — edit the highlighted rule. Only enabled for session and
  for *user-writeable* files (i.e. user / project / local; not
  managed, not default). Opens the same sub-prompt with current
  values populated. The save writes the *whole* canonical TOML to
  preserve formatting and comments.
- **`d`** — delete the highlighted rule. Same scope guardrails as
  `e`. Confirmation toast: "deleted rule #N (project); undo with u".
- **`p`** — promote a session rule into a file. Opens the scope
  picker (user / project / local). Removes from the runtime store
  on success.
- **`t`** — open a test pane: enter a tool name + a JSON input;
  shows the matched rule + provenance and the live decision under
  the current mode. Doesn't fire the tool — just runs the matcher.
- **Enter** — show the matched-rule detail for the highlighted row
  (pattern, source, file path, comment, reason, last-fired
  timestamp from the decision log).
- **`/`** — substring filter on pattern text.
- **`↑/↓`** in *Edit* tab only — reorder the rule within its scope
  by rewriting the file (move-within-file is just a position swap
  inside the array; preserves comments and other rules).

The **Audit** tab is a paginated viewer of the decision log
(`caliban perms audit` rendered in the TUI), filterable by tool
name, action, and matched-rule source. Each row links back to the
matching rule in the Edit tab via Enter.

## `caliban perms` CLI

Subcommands (clap derive; each command honors `--scope <s>` where
applicable):

| Subcommand                                                       | Behavior                                                                                                                                                       |
|------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `caliban perms list [--scope <s>] [--effective] [--json]`        | List rules in one scope, or the fully-merged effective set with `[scope]` chips. `--json` for scripting.                                                       |
| `caliban perms test <tool> [<input-json>]`                       | Run the matcher. Prints the matched rule (pattern, source, action, comment). Exit code: `0` allow, `1` deny, `2` ask, `3` error.                              |
| `caliban perms explain <tool> [<input-json>]`                    | As `test`, but additionally prints every rule that *would have* matched (in source order) and shows which mode overrides apply.                                |
| `caliban perms add <pattern> <action> [--scope <s>] [--comment <c>] [--reason <r>]` | Atomic append to the chosen scope's `permissions.toml`. Default scope: project.                                                                                |
| `caliban perms remove (--index <n> \| --pattern <p>) [--scope <s>]` | Atomic rewrite of the chosen scope's file with the matching rule removed. Idempotent when not found (exit `0`, print "no match").                              |
| `caliban perms import --from <path> [--scope <s>] [--dry-run]`   | Detects shape (legacy caliban TOML, legacy caliban JSON, Claude Code `settings.json`, Codex `config.json`) and writes canonical TOML to the chosen scope.       |
| `caliban perms export [--scope <s>] [--format toml\|json]`       | Print one scope's rules in canonical form, or `--format json` for handoff to a Claude Code-style consumer.                                                     |
| `caliban perms audit [--since <when>] [--tool <name>] [--action <a>] [--head <N>]` | Read the decision log; pretty-print or `--json` for scripting.                                                                                                 |
| `caliban perms lint [--scope <s>]` *(reserved; v2 stub only)*    | Surface basic shadowing warnings — strict-subset patterns later in the list. v2 implements only "duplicate exact pattern" detection; richer linting is v3.     |

The CLI shares `caliban-settings::writer` with the TUI; both go
through the same flock-protected atomic-write path.

## Hardening

### `permissions.enforce`

A boolean set in `permissions.toml` at any scope. Precedence:
managed `enforce = true` is sticky (cannot be overridden lower);
project `enforce = true` is sticky against user / local. User can
still set `enforce = true` for their own protection, overridable by
project (a project file can intentionally set `enforce = false` to
allow ergonomic local development).

Effects when `enforce = true` at the effective scope:

- `--no-permissions` aborts startup with a clear message naming the
  scope that set the flag.
- Cycling `Shift+Tab` into `bypassPermissions` shows a "blocked by
  enforce" toast and reverts to `default`, even when
  `--allow-dangerously-skip-permissions` is on the command line.
- `--auto-allow` (no-TTY fallback) aborts startup the same way.
- Starting with `default_mode = "bypassPermissions"` is hard-error
  the same way.

### Persistent decision log

A `DecisionRecorder` Hooks impl wraps `PermissionsHook` (via the
existing `CompositeHooks` chain). On every Allow or Deny outcome —
including outcomes that came from a runtime rule, a config rule, a
mode override, or the modal — it appends a JSONL line to
`$XDG_STATE_HOME/caliban/permission-decisions.jsonl` (or the OS
equivalent — `~/Library/Application Support/Caliban/state/` on
macOS, `%LOCALAPPDATA%\Caliban\state\` on Windows).

Line schema:

```json
{
  "ts": "2026-05-31T14:23:45Z",
  "session_id": "01J9X…",
  "turn_index": 12,
  "tool_use_id": "toolu_…",
  "tool_name": "Bash",
  "input_excerpt": "cargo test --all --workspace",
  "action": "allow",
  "matched_rule": {
    "pattern": "Bash:cargo *",
    "action":  "allow",
    "source":  { "scope": "project", "file": "/path/.caliban/permissions.toml", "index": 2 },
    "comment": "cargo ops"
  },
  "mode": "default"
}
```

`input_excerpt` is truncated to 256 chars and sanitized (no embedded
newlines; ` `-stripped). Tool-call inputs are *not* stored in
full — the log is a decision audit, not a session replay.

Rotation:

- Size: when the active file exceeds 100MB, it is renamed to
  `permission-decisions-<YYYY-MM-DD>.jsonl.gz` (gzipped in-place),
  and a fresh file starts.
- Age: a daily background sweep gzips files older than 30 days and
  deletes gzipped files older than 365 days.
- Opt-out: `permissions.audit_log = false` in `permissions.toml`
  disables both append and rotation.

### Modal teaches narrow

Covered above under "Modal writeback (P1)". The key behaviors:

1. Suggested patterns are ordered broadest → narrowest; selection
   defaults to *narrowest*.
2. The custom-pattern editor shows a live "would match this pending
   input? yes/no" preview.
3. The save defaults to project (commits to source for team review),
   making "I changed our team policy" a deliberate gesture.

### Always-visible bypass chip + drop keybind

Today the chip is rendered only when in `bypassPermissions` mode.
v2: when `bypass_latch` is set for the session, a *latch chip* is
rendered in the status bar at all times — red+bold, text
`⚠ bypass latched` — regardless of the current mode. The chip's
purpose is to make the latched state impossible to forget after the
operator cycles out of bypass mode (which previously hid the chip).

New keybind `Ctrl+Shift+B` ("drop bypass latch") clears the latch
mid-session. After clearing: re-entering bypass mode requires a new
`--allow-dangerously-skip-permissions` flag, which means a restart.
The chip disappears on drop with a confirming toast.

## Migration & compatibility

### Inputs accepted

For one minor release after this lands, *every* legacy input is read
without operator action:

- Legacy caliban `permissions.toml` `[[rule]] tool=…` form — read,
  WARN once with "deprecated; will be rewritten in v2 canonical form
  on next edit".
- Legacy caliban `settings.json` `permissions.{allow,ask,deny}` form
  — read, WARN once.
- Foreign agent JSON (Claude Code's `~/.claude/settings.json`,
  Codex's `~/.codex/config.json`) — only read when the operator runs
  `caliban settings import --from <path>`; never auto-discovered.

After two minor releases the legacy *write* paths are removed (we
were already at "deprecated" on `permissions::load_rules_file` per
`caliban-agent-core/src/permissions.rs:288`). After three, legacy
*read* paths are removed; only the canonical TOML schema is loaded.

### Operator workflows

- *Brand-new install* — TOML out of the gate, no migration.
- *Existing TOML user* — file continues to load. First caliban-owned
  edit (modal save, `caliban perms add`) silently upgrades the file
  to v2 canonical form, preserving comments and order.
- *Existing JSON user* — file continues to load with the WARN.
  Operator runs `caliban settings import --from
  ~/.config/caliban/settings.json` → produces `settings.toml`
  alongside. The JSON file can be deleted manually; next caliban
  start picks up the TOML only.
- *Importing a Claude Code config* — `caliban settings import --from
  ~/.claude/settings.json --scope user` extracts the supported keys
  (permissions, hooks, mcp, model, etc.) and writes them to the
  user-scope TOML.
- *Team that wants to commit a rule policy* — drop a
  `.caliban/permissions.toml` in the repo. Operators on the team see
  it merged at project scope; rules show `[project]` chips in
  `/permissions`.

## Public API sketches

```rust
// crates/caliban-agent-core/src/permissions.rs (changes)

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub pattern: String,
    pub action:  Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason:  Option<String>,    // deny-only; surfaces to the model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>, // reserved, unused in v2
}

/// Resolve a tool input against a rule list, returning the matched
/// rule when any. Used by both `PermissionsHook` and `caliban perms
/// test`/`explain`.
pub fn evaluate_rules<'a>(rules: &'a [Rule], ctx: &ToolCtx<'_>) -> Option<&'a Rule>;

/// Pattern grammar extensions live behind a single matcher:
pub mod matcher {
    pub fn matches(pattern: &str, ctx: &ToolCtx<'_>) -> bool;
    pub fn workspace_root() -> std::path::PathBuf;
    pub fn normalize_path(p: &str, root: &std::path::Path) -> std::path::PathBuf;
}
```

```rust
// crates/caliban-agent-core/src/decision_log.rs (new)

pub struct DecisionRecorder {
    inner: Arc<dyn Hooks>,
    writer: Arc<DecisionLogWriter>,    // owns the JSONL file + rotation
    enabled: bool,
}

#[async_trait]
impl Hooks for DecisionRecorder { /* before_tool wraps inner; logs decision */ }

pub struct DecisionLogWriter { /* lock-free append via a tokio mpsc + background task */ }

pub fn decision_log_path() -> std::path::PathBuf; // XDG-aware
```

```rust
// crates/caliban-settings/src/writer.rs (new)

pub struct ScopedFile {
    scope: Scope,
    path:  PathBuf,
    _lock: fs2::FileLock,
}

impl ScopedFile {
    pub fn acquire(scope: Scope, kind: FileKind) -> Result<Self>;
    pub fn read_settings(&self) -> Result<Settings>;
    pub fn rewrite(self, new: &Settings) -> Result<()>;   // atomic tmp+rename
    pub fn append_rule(self, rule: &Rule) -> Result<()>;
}
```

## Testing strategy

~38 enumerated tests grouped by area:

### Schema & loader

1. **TOML canonical form round-trips** — read v2 `permissions.toml`,
   re-emit, byte-identical (modulo trailing newlines).
2. **Legacy TOML form loads under `tool` alias** — `[[rule]] tool="X"`
   parses into `Rule { pattern: "X", … }`.
3. **Legacy JSON `allow/ask/deny` arrays flatten in deny→ask→allow order.**
4. **`deny_unknown_fields` on `Rule` rejects typos** with line+col.
5. **`.toml` wins over `.json` in the same scope; WARN logged once.**
6. **`.toml`-then-`.json` only-`.json` triggers the legacy-WARN.**
7. **`enforce` setting sticks managed > project > user > local.**
8. **`expires_at` field is accepted at parse time but ignored at evaluation
   time (deferred to v3).**

### Pattern grammar

9. **Globstar `Edit:**/*.rs`** matches `src/foo.rs`, `crates/x/src/y.rs`;
   does not match `target/foo.txt`.
10. **Path normalization** — `Edit:./src/foo.rs` matches an absolute
    `/repo/src/foo.rs` when workspace root is `/repo`.
11. **`Bash:~rm *`** matches `rm -rf x`, `sudo rm x`, `bash -c "rm x"`,
    `command rm x`. Does not match `lsbrm`.
12. **Dotted-key MCP arg** — `mcp__github__create_issue:repo=anthropic/*`
    matches `{ "repo": "anthropic/caliban" }`; does not match
    `{ "repo": "openai/foo" }`.
13. **Multi-kv AND** — `tool:a=1,b=2` requires both; either-missing → no
    match.
14. **First-arg fallback preserved** — `Bash:git *` (v1 form) matches
    `git push` and not `gitk`.
15. **`*` catch-all matches every tool**, including unknown MCP tools.
16. **Glob `?` matches exactly one character.**

### Modal writeback

17. **`Y` opens sub-prompt, doesn't write on `esc`** — cancel branch
    fires `Allow` once, no rule added.
18. **`Y` write to project appends a `[[permissions.rules]]`** —
    verifies file-on-disk shape, fsync, atomic rename.
19. **Two concurrent `Y` writes serialize via flock; both rules
    survive.** Uses two threads + a `tempdir`.
20. **External edit between modal open and save → writer detects diff,
    surfaces the conflict toast, re-runs the modal preview.**
21. **`N` always-deny writes `action="deny"` and the optional
    `reason` field; the next matching call returns the reason to the
    model.** Uses the `NonInteractiveAskHandler` shape with a stubbed
    deny path.
22. **Narrow-default selection** — modal default-highlights the
    narrowest suggestion, not the broadest.
23. **Custom-pattern live preview** — typed pattern is evaluated against
    the pending input; UI state mirror updates.

### `/permissions` overlay

24. **Edit tab `a`dd round-trips through the writer** — overlay test
    harness, no real TUI.
25. **`d`elete is disabled on `managed` and `default` rows.**
26. **`p`romote moves a session rule to project; runtime store loses
    it; file gains it.**
27. **`t`est pane evaluates pattern against the entered tool+input.**
28. **Watcher-driven external edit refreshes the list within 500ms.**

### CLI

29. **`caliban perms test Bash '{"command":"rm -rf /"}'` exits non-zero
    with `deny`** for the default rule set + a `Bash:~rm *` deny rule.
30. **`caliban perms add Bash:foo allow --scope project` writes the
    rule and is idempotent on re-run** (no duplicate appended; warn).
31. **`caliban perms import --from <claude-code-json>` produces a
    TOML file that, when loaded, matches the source's allow/ask/deny
    lists.**
32. **`caliban perms audit --tool Bash --since 1d` reads JSONL and
    filters correctly.**

### Hardening

33. **`enforce = true` + `--no-permissions` aborts at startup with a
    message naming the scope.**
34. **`enforce = true` + cycle into `bypassPermissions` → toast +
    revert, even with `--allow-dangerously-skip-permissions`.**
35. **Decision log writes on Allow and on Deny; rotates at the
    configured size cap; gzip filename matches the pattern.**
36. **Decision log honours `audit_log = false` — no file is created.**
37. **Bypass latch chip stays visible after cycling out of bypass mode
    when the latch is on; vanishes on `Ctrl+Shift+B`.**
38. **Dropping the latch then attempting to cycle into bypass without
    a fresh `--allow-dangerously-skip-permissions` results in the
    "requires flag" toast.**

Plus 2 TUI snapshot tests (overlay Edit tab, modal sub-prompt) under
the existing `caliban-tui` test harness.

## Risks

- **TOML serializer formatting drift** — the `toml` crate's serializer
  is opinionated about table ordering and inline-table choice. We
  mitigate by going through a hand-written canonical-form emitter
  for the `[[permissions.rules]]` array (so the v2 rewrite of a
  legacy file produces a stable layout), and pin a snapshot test of
  the emitted form.
- **flock on shared filesystems** — `fs2::FileExt::lock_exclusive`
  on NFS / SMB can be racy. We document this; operators with
  permissions files on a network share get a "best-effort locking;
  use `caliban perms add` from one machine at a time" note in the
  README. The decision log is local-only (`$XDG_STATE_HOME`), so
  network FS doesn't affect audit.
- **Concurrent modal vs watcher edit-loop** — when the modal writes
  a rule, the watcher fires and the rule set re-loads. If a second
  prompt fires *during* the watcher's debounce window, the matcher
  state could briefly reflect the older rule set. Mitigation: the
  modal save *also* calls `RuntimeRuleStore::add` for the same rule
  with a short TTL so the runtime store covers the watcher-debounce
  window. This is a minor in-process echo; it doesn't change disk.
- **JSON Schema and taplo schema drift apart** — both are
  build-emitted from the same `schemars`-instrumented struct. Schema
  generation is part of CI; a mismatch fails the build.
- **Pattern grammar gets denser** — `Bash:~rm *` is a small new
  syntactic detail; `key=glob` is a bigger jump. We mitigate by
  documenting clearly in `permissions.toml` examples and by ensuring
  the `caliban perms test`/`explain` subcommands are the
  recommended "is this rule what I think it is?" tool.
- **Decision log disk usage** — even with 100MB rotation, busy CI
  sessions could fill space. Mitigation: `audit_log = false` is a
  one-line off-switch, prominently documented; `cleanup_period_days`
  honored from existing settings.
- **TOML escape hatch loss** — JSON's `extra` flatten field is the
  "forward-compat catch-all" today. The TOML equivalent must do the
  same: unknown top-level keys land in `Settings::extra`. Test #4
  pins this for the per-rule struct; an analogous test pins it for
  the top-level `Settings`.
- **Parity-matrix regression** — the "Layered settings" row is
  currently ✅ in `docs/parity-gap-matrix.md`. v2 makes it *more*
  correct (TOML-primary; closes the comments-lost and source-order
  gaps). Acceptance criteria explicitly require the row stays ✅
  with updated notes.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all -- --check`
  clean.
- ≥38 new tests under
  `crates/caliban-agent-core/src/{permissions,decision_log}.rs`,
  `crates/caliban-settings/src/{writer,import}.rs`, and
  `caliban/src/{tui/ask, tui/permissions_overlay, perms_cli}.rs`;
  all passing.
- Generated JSON Schema and taplo schema both emitted under
  `target/schemas/` and CI-published.
- New `caliban-settings::writer` writes only `.toml`; `git grep
  '\.json"' crates/caliban-settings/src/writer.rs` is empty.
- `caliban perms list / test / explain / add / remove / import /
  export / audit / lint` subcommands wired, with help text.
- `/permissions` overlay's Edit tab is usable end-to-end in a
  manual run (covered by snapshot tests; manual confirmation noted
  in PR description).
- Decision log file appears under `$XDG_STATE_HOME/caliban/`; rotates
  in fixture tests at the configured cap.
- `permissions.enforce = true` in a fixture project aborts a
  `--no-permissions` startup with the expected message.
- `docs/parity-gap-matrix.md` updates:
  - Row "Permissions modes …" stays ✅ (no regression).
  - Row "Layered settings …" stays ✅, notes updated to call out
    TOML-primary with JSON-import.
  - New row (if not already present) under section A or M for the
    `/permissions` editor + `caliban perms` CLI — initial value ✅
    with this spec's PR series.
- README's "Permissions" section rewritten to document the v2 TOML
  schema, modal writeback, the CLI, `enforce`, the audit log, and
  the bypass-chip behavior. Old `permissions.toml` example file at
  `docs/examples/permissions.example.toml` updated to v2 form.
- A new ADR (`docs/adr/0034-permissions-v2-and-toml-primary-config.md`)
  in `accepted` status alongside this implementation; refines ADR
  0026 explicitly under its "Status" header.

## Cross-spec dependencies

- **ADR 0020 (Permission rules)** is the load-bearing v1 substrate.
  v2 extends, does not replace. The `Hooks::before_tool` integration
  point is unchanged.
- **ADR 0026 (Settings layering)** — refined, not invalidated.
  Layering / merge rules / scopes all stay. Only the canonical write
  format flips.
- **ADR 0029 (Permission modes + auto-mode)** — `ModeFilter` and
  `SharedPermissionMode` are unchanged. `enforce` interacts with
  mode cycling (refused into bypass). `default_mode` in the
  permissions table is the new home for what ADR 0029 called
  "`defaultMode` in settings".
- **ADR 0023 (MCP v2)** — per-server `[server.X.permissions]` blocks
  continue to layer in via the existing path; they now compose with
  the v2 rule grammar (so `mcp__github__create_issue:repo=…/*`
  works for both globally-configured and per-server rules).
- **ADR 0021 (Sub-agents)** — sub-agents inherit the parent's rule
  set + `SharedPermissionMode`. v2 doesn't change subagent
  semantics; per-subagent overrides remain a v3 follow-up.
- **`writing-plans` skill** is the next step: this spec hands off
  for an implementation plan covering ~5 PR series boundaries
  (TOML-primary loader/writer; pattern grammar; modal writeback;
  `/permissions` editor; `caliban perms` CLI; hardening +
  audit log; docs/README + parity matrix update).
