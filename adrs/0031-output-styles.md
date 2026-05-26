# ADR 0031 · Output styles

- **Status:** accepted
- **Date:** 2026-05-24
- **Author:** john.ford2002@gmail.com
- **Spec:** `docs/superpowers/specs/2026-05-24-output-styles-design.md`
- **Depends on:** ADR 0018 (memory tier model — splice pattern reused),
  ADR 0019 (skills — frontmatter parser pattern reused), ADR 0030
  (plugin packaging — plugin-supplied styles).

## Context

Claude Code exposes four built-in output styles — `Default`,
`Proactive`, `Explanatory`, `Learning` — plus a custom-style file
format with frontmatter (`name`, `description`, `keep-coding-instructions`,
`force-for-plugin`). Styles modify the system prompt only; they're
orthogonal to permission mode, tools, and hooks. Operators activate
via `/config → Output style` or the `outputStyle` setting. Caliban
currently has none of this surface (matrix row L is 🔴 across the
board).

## Decision

### Output styles are markdown files with frontmatter, like skills

A custom style is a single `.md` file with a YAML frontmatter block
declaring `name`, `description`, `keep_coding_instructions` (bool,
default `true`), and `force_for_plugin` (bool, default `false`). The
body is the prompt block. The parser reuses `serde_yaml` (already in
the workspace for skills) and mirrors `caliban-skills`'s frontmatter
shape.

We use snake_case (`keep_coding_instructions`, `force_for_plugin`)
internally; the loader accepts kebab-case (`keep-coding-instructions`,
`force-for-plugin`) as aliases for Claude-Code-format compatibility.

### A new crate `caliban-output-styles` holds it

Modeled on `caliban-skills`: loader + struct + tool-adjacent pieces.
It owns `OutputStyle`, `OutputStylePrefix`, `default_roots`,
`load_styles`, `select_active`, and the `Learning` post-processor.
Built-in style bodies live as `include_str!`'d markdown files under
`crates/caliban-output-styles/src/builtins/`.

### Discovery roots and shadowing

Same shape as skills: project > user > plugin > built-in. Project
styles at `<workspace>/.caliban/output-styles/<name>.md` shadow user
styles at `$XDG_CONFIG_HOME/caliban/output-styles/<name>.md`, which
shadow plugin-supplied styles (which are namespaced
`<plugin>:<name>`), which shadow the four built-ins.

### The splice pattern is reused from `MemoryPrefix`

`OutputStylePrefix::splice_into(base)` wraps the active style's body in
`<output-style name="...">…</output-style>` and prepends to `base`. It
composes with `MemoryPrefix::splice_into`: memory tiers go first, then
the output-style block, then the base body. The `Default` style is the
no-op — it emits no block at all, so switching to `Default` produces
the exact same prompt as having no style configured. This minimizes
prompt-cache invalidation for operators who never customize.

### Style activation requires `/clear` or restart

System prompts are cached by every major provider. Live-swapping the
style mid-session would invalidate caches without warning and produce
inconsistent assistant behavior. The `/config → Output style` overlay
surfaces a "applies after /clear or restart" hint; the in-memory
selection updates, but the system prompt that the provider sees does
not change until the next session.

### The `Learning` style is the only style that touches assistant text

`Learning` instructs the model to emit `TODO(human): <prompt>` markers
on non-trivial decisions; a post-processor (the new
`AssistantPostProcessor` trait in `caliban-agent-core`) tags those
markers in the assistant's output so the TUI can highlight them.
`Default`, `Proactive`, and `Explanatory` install an identity
post-processor. Tools, hooks, and message contents are unaffected.

### `force_for_plugin: true` lets a plugin pin its style

A plugin-supplied style with `force_for_plugin: true` overrides the
operator's `output_style` setting while the plugin is enabled. The
`/config` picker shows a "locked by plugin: X" badge. Disabling the
plugin releases the lock and the operator's selection returns. Bare
(non-plugin) styles with `force_for_plugin: true` are ignored — only
plugin-sourced styles honor the flag.

## Consequences

- **Positive:** Closes matrix row L (both rows) with a single
  small-footprint crate that reuses two existing patterns (memory
  splice + skills frontmatter parse). Plugin-supplied styles fit
  naturally into the namespacing already proposed in ADR 0030. The
  `keep_coding_instructions: false` knob unlocks
  documentation-/writing-only modes without a separate "agent mode"
  feature.
- **Negative:** Adds a new crate. Frontmatter parsing duplicated
  between `caliban-skills` and `caliban-output-styles` (deferred:
  factor out a `frontmatter` helper in `caliban-core` once a third
  consumer appears). Prompt-cache invalidation is the operator's
  responsibility on style switch — surfaced via a hint, but still a
  papercut. The `Learning` post-processor adds a small per-turn cost
  even when the marker scan finds nothing.
- **Revisit if:** Operators want streaming-time style mutation
  (today the post-processor runs after streaming completes). Style
  composition becomes a real ask (today only one style is active). A
  community style library justifies bundling a marketplace pointer in
  defaults.
