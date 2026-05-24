# Permissions v1 Implementation Plan

> Executed inline. Short plan for the record.

**Goal:** Glob-based, Hook-layered tool gating with TOML rule files and `Allow/Deny/Ask` actions. Built-ins ask before `Bash/Write/Edit/WebFetch` unless overridden.

**Architecture:**
- `caliban_agent_core::permissions` exposes `Action`, `Rule`, `PermissionsHook`, `AskHandler` trait, `NonInteractiveAskHandler`, `matches_glob`, `default_rules()`, `load_rules`, `load_rules_file`.
- `PermissionsHook: Hooks` wraps an inner `Hooks` impl so it composes; gates `before_tool` only; pass-through on the other lifecycle methods.
- Rule patterns: `Tool` or `Tool:first-arg-glob`. Glob = `*` (zero or more) and `?` (one). First-arg accessors are baked in for `Bash` (`command`), `WebFetch` (`url`), `Read/Write/Edit` (`path`); unknown tools have no first-arg, so only bare-name patterns match.
- Built-in defaults: `Read/Grep/Glob/TodoWrite/EnterPlanMode/ExitPlanMode` → Allow; `WebFetch/Bash/Write/Edit/*` → Ask.
- Rule sources (high→low): CLI flags, `<workspace>/.caliban/permissions.toml`, `~/.config/caliban/permissions.toml`, defaults. First-match-wins inside the merged list.
- TUI's interactive Ask modal is **deferred** to a follow-up PR; v1 ships with `NonInteractiveAskHandler` which converts Ask → Deny unless `--auto-allow` is set (then Ask → Allow, with help text warning).

**Tests delivered (16):**
- Glob matcher: `*` matches anything, `?` matches one, literal pattern, prefix glob, the `Bash:rm *` ≠ `sudo rm` case.
- Defaults: Read allowed, Bash asks, WebFetch asks.
- First-match-wins inside the merged source.
- CLI rule beats default.
- `*` catch-all matches unknown tools.
- First-arg patterns require a known accessor.
- Async hook: Deny action returns Deny decision, Ask without auto-allow denies, Ask with auto-allow allows.
- TOML loader: valid file parses, missing file returns empty, invalid action errors.

**CLI flags added:**
- `--no-permissions` (env `CALIBAN_NO_PERMISSIONS`) — disables gating entirely.
- `--allow PAT` / `--deny PAT` / `--ask PAT` — top-priority rule lists.
- `--auto-allow` (env `CALIBAN_AUTO_ALLOW`) — non-interactive Ask → Allow.

**Files:**
- create `crates/caliban-agent-core/src/permissions.rs`
- modify `crates/caliban-agent-core/Cargo.toml` (add `toml`, `dirs`, dev-dep `tempfile`)
- modify `crates/caliban-agent-core/src/lib.rs` (re-exports)
- modify `caliban/src/main.rs` (flags + builder wiring)
- create `docs/examples/permissions.example.toml`

**Spec:** `docs/superpowers/specs/2026-05-23-permissions-design.md`
**Deferred:** TUI Ask modal overlay; `--ask=stdin` mode; shadowed-rule detection.
