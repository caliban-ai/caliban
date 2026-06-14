# Proactive skill invocation (issue #56)

**Status:** Approved design — ready for implementation
**Issue:** caliban-ai/caliban#56
**Date:** 2026-06-13

## Problem

A skill copied into `.caliban/skills/<name>/SKILL.md` loads correctly (it appears
in `/skills`), but the model does **not** proactively invoke it when a task
matches the skill's description — it improvises instead, so the skill is
effectively never activated.

This is a capability/UX gap, **not** a loader defect. Skills reach the model
through exactly one channel: the `Skill` tool's `description()` — a bulleted
`- name: first-line` list (`crates/caliban-skills/src/tool.rs::build_description`).
Nothing instructs the model to *consult* that list before acting. Claude Code's
superpowers pack relies on a SessionStart injection that mandates checking for an
applicable skill before acting; caliban has no equivalent.

## Goal

Make the model reach for a matching loaded skill **before** improvising, via two
reinforcing nudges:

1. A stronger `Skill` tool description.
2. A compact "skills awareness" section injected into the system prompt at
   session start.

On by default whenever skills are actually loaded; suppressible via config; zero
cost when no skills are loaded.

## Design

### 1. Stronger `Skill` tool description

`crates/caliban-skills/src/tool.rs::build_description` — prepend a guidance
sentence to the existing intro, before the bulleted skill list:

> "If a listed skill matches the task at hand, invoke it before improvising."

Counted against the existing `DESCRIPTION_BUDGET_BYTES` (8 KiB) so list
truncation continues to respect the cap.

### 2. System-prompt skills-awareness section

`caliban/src/system_prompt.rs` — new function mirroring `append_todo_block`:

```rust
pub(crate) fn append_skills_block(prompt: &str, skill_names: &[&str]) -> String
```

- Empty `skill_names` → returns `prompt` unchanged.
- Otherwise appends:

  ```
  ## Skills
  Loaded skills extend your abilities. BEFORE acting on a task, check whether a
  loaded skill applies; if one matches, invoke Skill({name}) FIRST rather than
  improvising.

  Available: <name>, <name>, ...
  ```

- The joined name list is capped at a modest budget (2 KiB); overflow is
  truncated with `, …` so a pathological number of skills cannot bloat the
  prompt.
- Names only — the authoritative first-line descriptions stay in the tool
  description (single source of truth, avoids doubling token cost).

### 3. Extracting skill names from the registry

The `Skill` tool already holds the loaded skills. Rather than re-walking the
filesystem (drift risk) or threading a `Vec<String>` through `build_registry`'s
return type (ripples to callers), recover the concrete type via a downcast:

- `crates/caliban-agent-core/src/tool.rs` — add to the `Tool` trait a defaulted
  hook (returns `None`, so none of the 34 existing impls change):

  ```rust
  fn as_any(&self) -> Option<&dyn std::any::Any> { None }
  ```

- `crates/caliban-skills/src/tool.rs` — `SkillTool` overrides it
  (`Some(self)`) and gains:

  ```rust
  pub fn skill_names_sorted(&self) -> Vec<&str>
  ```

### 4. Injection point

`caliban/src/startup.rs::resolve_system_prompt` is the single place the prompt is
built and layered (called once at startup; skills are static for the session).
Inject the skills block on the resolved prompt for **both** the default and the
custom `--system` / `--system-file` paths (only `--no-system`, which yields no
prompt at all, is skipped). The block is appended at the tail, after the
output-style and memory layering, so it cannot be dropped when an output style
replaces the coding-instructions body.

Gating (pure, unit-testable):

- `settings.tools.skill_guidance == Some(false)` → inject nothing.
- No `Skill` tool registered (`--no-skills` / `--bare` / `--no-tools`) or no
  skills loaded → empty name list → no section.

### 5. Config

`crates/caliban-settings/src/settings.rs` — add to `ToolsConfig`:

```rust
/// Inject proactive skill-invocation guidance into the system prompt when
/// skills are loaded. `None`/`true` = on (default), `false` = off.
pub skill_guidance: Option<bool>,
```

TOML:

```toml
[tools]
skill_guidance = false   # disable the nudge
```

## Behavior notes

- Built-in skills (auto-memory) keep the section non-empty in normal sessions;
  `--no-skills` / `--bare` register no `SkillTool`, so the section is omitted.
- Applies to custom system prompts too — proactive skill use is useful
  regardless of who authored the base prompt. `--no-system` opts out entirely.

## Testing

- `tool.rs`: `build_description` contains the new guidance sentence; budget still
  respected; `skill_names_sorted` returns names in sorted order.
- `system_prompt.rs`: `append_skills_block` — empty → unchanged; non-empty →
  section header + `Available:` + names; long list truncates with `…`.
- `settings.rs`: `{"tools":{"skill_guidance":false}}` parses to `Some(false)`;
  absent key → `None`.
- Gating helper: pure function returns an empty list when `skill_guidance` is
  `Some(false)`, the names otherwise.

## Out of scope

- **General SessionStart context-injection hook surface** (ticket direction #3):
  a larger, reusable feature; not needed for this fix.
- **Name-vs-directory silent-skip diagnostics:** the ticket itself calls for a
  **separate** diagnostics issue (a misnamed skill is currently skipped without a
  warning). Filed as a follow-up, not part of this PR.
