# ADR 0040 · Slash command registry

- **Status:** proposed
- **Date:** 2026-05-24
- **Spec:** `docs/superpowers/specs/2026-05-24-slash-command-coverage-design.md`

## Context

caliban currently has four hard-coded slash commands (`/plan`,
`/memory`, `/skills`, `/quit`) dispatched from a `match` in
`Tui::handle_slash_command`. Closing the parity gap with Claude Code
adds another ~24 commands at minimum, plus plugin-supplied commands.
Continuing the match arm pattern is untenable: it forces every command
into one file, prevents plugins from registering commands, and
duplicates the typeahead suggester data.

## Decision

### A `SlashCommand` trait + central `SlashCommandRegistry`

Each slash command becomes its own `impl SlashCommand` in
`caliban/src/tui/slash/<group>.rs`. The registry holds them by name in
a `HashMap<&'static str, Arc<dyn SlashCommand>>` and exposes
`register`, `suggest`, `dispatch`. The TUI's input bar consults the
suggester for typeahead; the dispatcher routes execution.

### A shared `SlashCtx<'a>` is passed to every command

Commands need mutable access to the running session and immutable
references to long-lived registries (providers, router, MCP manager,
skills, hooks, sub-agent fleet, settings). Threading each separately
into every command would mean re-plumbing nine call sites every time a
new shared resource is added. Instead, `SlashCtx` is a single
borrowing struct constructed per command dispatch. Commands take
`&mut SlashCtx<'_>` and reach in for what they need.

The risk is `SlashCtx` becoming a god-object. We accept that risk and
commit to splitting it if it grows past ~20 fields.

### Slash commands are operator UI, not model tools — no permission gating

Slash commands run as the operator's direct action; they are not gated
by the permission rule grammar that protects model-initiated tool
calls. Commands that wrap destructive operations (`/clear`,
`/rewind`-restore, `/logout`) implement their own interactive
confirmation in their overlay. This keeps the rule grammar focused on
its actual job (constraining the *model*) and removes a layer of
ambiguity ("did /clear get rejected by a Bash rule?").

### Hooks fire on slash submission

`UserPromptSubmit` (from ADR 0024 / Hooks expansion) fires *before* the
slash parser runs. Hook payload includes `is_slash: bool`, `command:
str`, `args: str`. A hook can reject or modify the slash command —
useful for audit logging or per-operator policy.

### Stubs are first-class

Several slash commands depend on machinery being designed in sibling
specs (settings, MCP v2, plugins, OTel/cost, checkpointing). Rather
than wait for everything to land, we register stubs that emit a
helpful status message ("cost tracking lands in PR #N — see
`docs/superpowers/specs/2026-05-24-otel-and-cost-design.md`"). The stub
files name the in-flight spec so the user can tell what's coming.

## Consequences

- **Positive:** Clean extension point — adding a command is one file
  and one `registry.register(...)` line. Plugins (per ADR 0030)
  register commands the same way. Typeahead works automatically for
  every registered command. `/help` enumerates the live set, so
  documentation never drifts from reality.
- **Negative:** `SlashCtx` is wide. Stubs can confuse operators if the
  message isn't clear. Plugin-supplied commands shadowing built-ins
  need consistent semantics (plugin loses); logged at registration
  time. Adds ~150 LOC of trait/registry plumbing for ~24 small command
  impls.
- **Revisit if:** Commands begin to need *session-specific* command
  registration (e.g. a sub-agent's command appears only when that
  sub-agent is attached). Today's registry is process-global; a
  per-session overlay can be added without breaking the trait.
